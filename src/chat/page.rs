//! The chat's page: one HTML document loaded once per pane, then updated by
//! messages (`kova.update(…)`) carrying only what changed.
//!
//! Every text from the transcript is escaped here, in Rust: what an agent
//! writes (or a web page it fetched) is data. Markdown goes through
//! pulldown-cmark with raw HTML shown as text, links kept only when http(s)
//! or mailto, images never loaded. The page's content security policy runs
//! only the script carrying this load's nonce, so markup that slipped
//! through still could not run anything; the script uses no inline handler.
//!
//! The layout follows KovaLink's readability rules
//! (`link/docs/13-chat-lisibilite.md`): the turn's state above the box, the
//! assistant's text full width without a bubble, the user's message in a
//! quiet bubble on the right, tool calls as compact one-line rows grouped
//! and folded past five, the last exchanges first, nothing folding by
//! itself.

use serde::Serialize;

use super::conversation::{BlockKind, Conversation, Item, Tool};
use super::links::linkify;

/// How many exchanges (a user message and what follows it) the page opens
/// on; `Show earlier` adds as many again.
pub const EXCHANGES_STEP: usize = 20;
/// A run of tool calls up to this length stays unfolded (rule 9).
pub const GROUP_FOLD_OVER: usize = 5;
/// The script message handler the page posts to.
pub const HANDLER: &str = "kovaChat";

/// One block of the thread as the page shows it, keyed by a stable id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub html: String,
}

impl Entry {
    /// The hash the page compares to keep a node it already has.
    pub fn hash(&self) -> u64 {
        fnv(&self.html)
    }
}

/// The design system's colours, as CSS values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub dark: bool,
    pub bg: String,
    pub bg2: String,
    pub t1: String,
    pub t2: String,
    pub t3: String,
    pub b2: String,
    pub b3: String,
    pub accent: String,
    pub working: String,
    pub awaiting: String,
    pub error: String,
    pub ok: String,
}

pub fn escape(s: &str) -> String {
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

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Markdown as an HTML fragment: raw HTML as text, a link only to http(s),
/// mailto or a local `.md` / `.html` file, no image source. A URL or such a
/// path in the text is a link too, but not inside code.
pub fn markdown(source: &str) -> String {
    use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag, TagEnd, TextMergeStream};
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    // Inside a link to another scheme: its text stays, the link goes.
    let mut unsafe_link = false;
    // Inside a link to a file: closed by hand, as it was opened.
    let mut path_link = false;
    let mut in_link = 0usize;
    let mut in_code = false;
    let parser = TextMergeStream::new(Parser::new_ext(source, options)).map(|event| match event {
        Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
        Event::Text(text) if in_link == 0 && !in_code => Event::InlineHtml(CowStr::from(linkify(&text))),
        Event::Start(Tag::CodeBlock(kind)) => {
            in_code = true;
            Event::Start(Tag::CodeBlock(kind))
        }
        Event::End(TagEnd::CodeBlock) => {
            in_code = false;
            Event::End(TagEnd::CodeBlock)
        }
        Event::Start(Tag::Link { link_type, dest_url, title, id }) => {
            let dest = dest_url.trim();
            let lower = dest.to_ascii_lowercase();
            if super::links::is_panel_path(dest) {
                path_link = true;
                in_link += 1;
                return Event::InlineHtml(CowStr::from(super::links::anchor(dest, super::links::Kind::Path)));
            }
            if !(super::links::is_web(dest) || lower.starts_with("mailto:")) {
                unsafe_link = true;
                in_link += 1;
                return Event::Text("".into());
            }
            in_link += 1;
            Event::Start(Tag::Link { link_type, dest_url, title, id })
        }
        Event::End(TagEnd::Link) if unsafe_link => {
            unsafe_link = false;
            in_link = in_link.saturating_sub(1);
            Event::Text("".into())
        }
        Event::End(TagEnd::Link) if path_link => {
            path_link = false;
            in_link = in_link.saturating_sub(1);
            Event::InlineHtml("</a>".into())
        }
        Event::End(TagEnd::Link) => {
            in_link = in_link.saturating_sub(1);
            Event::End(TagEnd::Link)
        }
        Event::Start(Tag::Image { link_type, title, id, .. }) => {
            Event::Start(Tag::Image { link_type, dest_url: "".into(), title, id })
        }
        other => other,
    });
    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

/// One thing of the thread, before it is shown or kept behind the scenes.
enum Unit<'a> {
    User { id: &'a str, text: &'a str },
    Text { key: String, text: &'a str },
    Tool(&'a Tool),
    Thinking { key: String, text: &'a str },
    Note { key: String, text: &'a str },
    Image { key: String },
    Behind { key: String, label: &'a str, text: &'a str },
}

/// The entries of the last `exchanges` exchanges, and whether earlier ones
/// exist. `working` says whether a tool call without its result is running
/// (or was stopped).
///
/// Only the user's messages and the assistant's final text (no tool call
/// after it in its turn) are shown. Everything between two of them (tool
/// calls, the text that goes with them, notes, other sessions' messages,
/// finished tasks) folds into one `N actions en coulisse` line. While the
/// agent works with no call running, a `Réflexion` line ends the thread.
pub fn entries(conv: &Conversation, exchanges: usize, working: bool) -> (Vec<Entry>, bool) {
    let users: Vec<usize> = conv.items.iter().enumerate().filter(|(_, i)| matches!(i, Item::User { .. })).map(|(n, _)| n).collect();
    let start = if users.len() > exchanges { users[users.len() - exchanges] } else { 0 };
    let mut units: Vec<Unit> = Vec::new();
    for item in conv.items.iter().skip(start) {
        match item {
            Item::User { id, text, .. } => units.push(Unit::User { id, text }),
            Item::Note { id, text } => units.push(Unit::Note { key: format!("n:{id}"), text }),
            Item::Behind { id, label, text } => units.push(Unit::Behind { key: format!("b:{id}"), label, text }),
            Item::Assistant { id, blocks, .. } => {
                for (k, block) in blocks.iter().enumerate() {
                    units.push(match &block.kind {
                        BlockKind::Text(text) => Unit::Text { key: format!("t:{id}:{k}"), text },
                        BlockKind::Tool(tool) => Unit::Tool(tool),
                        BlockKind::Image => Unit::Image { key: format!("i:{id}:{k}") },
                        BlockKind::Thinking(text) => Unit::Thinking { key: format!("k:{id}:{k}"), text },
                    });
                }
            }
        }
    }
    // A text is final when no tool call follows it before the next user message.
    let mut visible = vec![false; units.len()];
    let mut tool_after = false;
    for (n, unit) in units.iter().enumerate().rev() {
        match unit {
            Unit::User { .. } => {
                visible[n] = true;
                tool_after = false;
            }
            Unit::Tool(_) => tool_after = true,
            Unit::Text { .. } => visible[n] = !tool_after,
            _ => {}
        }
    }
    // Only the current turn's calls can be running: one from an earlier turn
    // that never got its result was stopped.
    let last_user = units.iter().rposition(|u| matches!(u, Unit::User { .. }));
    let mut out = Vec::new();
    let mut run: Vec<&Unit> = Vec::new();
    for (n, unit) in units.iter().enumerate() {
        if !visible[n] {
            // A thinking block without its text is nothing to show.
            if !matches!(unit, Unit::Thinking { text, .. } if text.is_empty()) {
                run.push(unit);
            }
            continue;
        }
        if !run.is_empty() {
            out.push(behind_entry(&run, working && last_user.is_none_or(|u| n > u)));
            run.clear();
        }
        out.push(match unit {
            Unit::User { id, text } => Entry {
                id: format!("u:{id}"),
                html: format!(r#"<div class="user"><div class="bubble">{}</div></div>"#, linkify(text)),
            },
            Unit::Text { key, text } => Entry { id: key.clone(), html: format!(r#"<div class="asst">{}</div>"#, markdown(text)) },
            _ => unreachable!("only messages are visible"),
        });
    }
    let running = working && run.iter().any(|u| matches!(u, Unit::Tool(t) if t.result.is_none()));
    // The reasoning going on, when the transcript has its text and nothing
    // came after it: shown on the Réflexion line, not behind the scenes.
    let mut thought = "";
    if working && !running && let Some(Unit::Thinking { text, .. }) = run.last() {
        thought = text;
        run.pop();
    }
    if !run.is_empty() {
        out.push(behind_entry(&run, working));
    }
    if working && !running {
        out.push(thinking_entry(thought));
    }
    (out, start > 0)
}

/// `N actions en coulisse`: the hidden things between two messages, folded,
/// compact once unfolded.
fn behind_entry(run: &[&Unit], working: bool) -> Entry {
    let key = |u: &Unit| match u {
        Unit::Tool(t) if !t.id.is_empty() => format!("g:{}", t.id),
        Unit::Tool(_) => "g:@".to_string(),
        Unit::Text { key, .. } | Unit::Thinking { key, .. } | Unit::Note { key, .. } | Unit::Image { key } | Unit::Behind { key, .. } => key.clone(),
        Unit::User { id, .. } => format!("u:{id}"),
    };
    let id = format!("c:{}", key(run[0]));
    let mut inner = String::new();
    let mut tools: Vec<&Tool> = Vec::new();
    let flush = |tools: &mut Vec<&Tool>, inner: &mut String| {
        if !tools.is_empty() {
            inner.push_str(&tool_group(tools, working, inner.len()).html);
            tools.clear();
        }
    };
    let mut states = Vec::new();
    for unit in run {
        if let Unit::Tool(t) = unit {
            states.push(state_of(t, working));
            tools.push(t);
            continue;
        }
        flush(&mut tools, &mut inner);
        match unit {
            Unit::Text { text, .. } => inner.push_str(&format!(r#"<div class="asst bt">{}</div>"#, markdown(text))),
            Unit::Note { text, .. } => inner.push_str(&format!(r#"<div class="note">{}</div>"#, escape(text))),
            Unit::Image { .. } => inner.push_str(r#"<div class="note">image</div>"#),
            Unit::Thinking { key, text } => inner.push_str(&fold_row(key, "Réflexion", text)),
            Unit::Behind { key, label, text } => inner.push_str(&fold_row(key, label, text)),
            Unit::Tool(_) | Unit::User { .. } => {}
        }
    }
    flush(&mut tools, &mut inner);
    let st = if states.contains(&State::Running) {
        r#"<span class="st run"></span>"#
    } else if states.contains(&State::Error) {
        r#"<span class="st err"></span>"#
    } else {
        ""
    };
    let n = run.len();
    let label = if n == 1 { "1 action en coulisse".to_string() } else { format!("{n} actions en coulisse") };
    Entry {
        html: format!(
            r#"<div class="behind" data-id="{eid}"><button class="trow bhead" data-act="fold"><span class="verb">{label}</span>{st}</button><div class="fold">{inner}</div></div>"#,
            eid = escape(&id),
        ),
        id,
    }
}

/// A folded row: its label, its text unfolded.
fn fold_row(key: &str, label: &str, text: &str) -> String {
    let body = if text.is_empty() { String::new() } else { format!(r#"<pre class="tres">{}</pre>"#, linkify(text)) };
    format!(
        r#"<div class="tool" data-id="{key}"><button class="trow" data-act="fold"><span class="verb">{label}</span></button><div class="fold">{body}</div></div>"#,
        key = escape(key),
        label = escape(label),
    )
}

/// `Réflexion` while the model thinks, a light sweeping across it; its text
/// unfolds on a click when the transcript has it.
fn thinking_entry(text: &str) -> Entry {
    let html = if text.is_empty() {
        r#"<div class="think"><span class="shimmer">Réflexion</span></div>"#.to_string()
    } else {
        format!(
            r#"<div class="think" data-id="r:live"><button class="trow thead" data-act="fold"><span class="shimmer">Réflexion</span></button><div class="fold"><pre class="tres">{}</pre></div></div>"#,
            escape(text)
        )
    };
    Entry { id: "r:live".to_string(), html }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Running,
    Stopped,
    Ok,
    Error,
}

fn state_of(tool: &Tool, working: bool) -> State {
    match &tool.result {
        Some(r) if r.is_error => State::Error,
        Some(_) => State::Ok,
        None if working => State::Running,
        None => State::Stopped,
    }
}

fn state_class(s: State) -> &'static str {
    match s {
        State::Running => "run",
        State::Stopped => "stop",
        State::Ok => "ok",
        State::Error => "err",
    }
}

fn tool_row(tool: &Tool, working: bool) -> String {
    let state = state_of(tool, working);
    let detail = tool.detail.as_deref().map(|d| format!(r#"<span class="tdet">{}</span>"#, escape(d))).unwrap_or_default();
    let result = match &tool.result {
        Some(r) if !r.text.is_empty() => {
            let cut = if r.truncated { r#"<div class="cut">cut, the full output is in the terminal</div>"# } else { "" };
            format!(r#"<pre class="tres">{}</pre>{cut}"#, escape(&r.text))
        }
        Some(_) => r#"<div class="cut">no output</div>"#.to_string(),
        None => String::new(),
    };
    format!(
        r#"<div class="tool" data-id="t:{id}"><button class="trow" data-act="fold"><span class="verb">{verb}</span><span class="target">{target}</span>{detail}<span class="st {st}"></span></button><div class="fold">{result}</div></div>"#,
        id = escape(&tool.id),
        verb = escape(&tool.verb),
        target = escape(&tool.target),
        st = state_class(state),
    )
}

fn tool_group(tools: &[&Tool], working: bool, at: usize) -> Entry {
    // A call without an id (never seen, but nothing forbids it) is keyed by
    // its place, so two of them do not share one node on the page.
    let id = if tools[0].id.is_empty() { format!("g:@{at}") } else { format!("g:{}", tools[0].id) };
    let rows: String = tools.iter().map(|t| tool_row(t, working)).collect();
    if tools.len() <= GROUP_FOLD_OVER {
        return Entry { id, html: format!(r#"<div class="tools">{rows}</div>"#) };
    }
    let states: Vec<State> = tools.iter().map(|t| state_of(t, working)).collect();
    let st = if states.contains(&State::Running) {
        State::Running
    } else if states.contains(&State::Error) {
        State::Error
    } else if states.contains(&State::Stopped) {
        State::Stopped
    } else {
        State::Ok
    };
    let last = tools[tools.len() - 1];
    Entry {
        html: format!(
            r#"<div class="tools group" data-id="{gid}"><button class="trow ghead" data-act="fold"><span class="verb">{n} actions</span><span class="target">{verb} {target}</span><span class="st {st}"></span></button><div class="fold">{rows}</div></div>"#,
            gid = escape(&id),
            n = tools.len(),
            verb = escape(&last.verb),
            target = escape(&last.target),
            st = state_class(st),
        ),
        id,
    }
}

/// A permission card's HTML: its message, its detail, its field, its
/// buttons. An ask's card is the Cockpit's (`cards::ask_card_html`).
pub fn card_html(card: &super::cards::Card) -> String {
    use super::cards::Act;
    let detail = card.detail.as_deref().map(|d| format!(r#"<pre class="cdet">{}</pre>"#, escape(d))).unwrap_or_default();
    let field = if card.input { r#"<textarea class="cfield" rows="1" placeholder="Answer"></textarea>"# } else { "" };
    let buttons: String = card
        .buttons
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let (act, extra) = match &b.act {
                Act::Answer { action } => ("answer", format!(r#" data-action="{}""#, escape(action))),
                Act::Open => ("open", String::new()),
                Act::Permission { choice } => (
                    "permission",
                    format!(r#" data-choice="{}""#, serde_json::to_value(choice).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()),
                ),
            };
            let class = if i == 0 { "cbtn primary" } else { "cbtn" };
            let needs = if b.needs_text { r#" data-needs-text="1""# } else { "" };
            format!(r#"<button class="{class}" data-act="{act}"{extra}{needs}>{}</button>"#, escape(&b.label))
        })
        .collect();
    let hint = card.hint.as_deref().map(|h| format!(r#"<div class="chint">{}</div>"#, escape(h))).unwrap_or_default();
    let kind = serde_json::to_value(&card.kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    format!(
        r#"<div class="pcard {kind}" data-id="{id}"><div class="cfrom">{from}</div><div class="cmsg">{msg}</div>{detail}{field}<div class="cbtns">{buttons}</div>{hint}</div>"#,
        id = escape(&card.id),
        from = escape(&card.from),
        msg = escape(&card.message),
    )
}

/// What one update tells the page.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Update {
    pub banner: Option<BannerWire>,
    pub working: bool,
    /// Every entry in order; `html` only for those the page does not hold.
    pub entries: Option<Vec<WireEntry>>,
    pub more: bool,
    pub cards: Option<Vec<WireEntry>>,
    /// The messages waiting in the queue, in order, when it changed.
    pub queued: Option<Vec<Queued>>,
    /// A one-off line under the box.
    pub note: Option<String>,
    /// Text to put back in the box (a message that was not sent).
    pub restore: Option<String>,
    /// Images just pasted: their chips in the box.
    pub attach: Option<Vec<Attach>>,
    /// The voice typing pill, when it changed.
    pub voice: Option<crate::voice::Pill>,
    /// Dictated text to insert at the cursor.
    pub dictated: Option<Dictated>,
    /// The page was just loaded: place the view on the last exchange.
    pub first: bool,
    /// Give the box the keyboard.
    pub focus: bool,
}

/// The banner as the page shows it: its state (a class) and its text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BannerWire {
    pub state: &'static str,
    pub text: String,
}

/// A dictation's clean text, and whether the box sends right after
/// (`[chat] voice_auto_send`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Dictated {
    pub text: String,
    pub send: bool,
}

/// A pasted image as the box shows it: its file, and a `data:` URL of it
/// when it is small enough to show.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Attach {
    pub path: String,
    pub src: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Queued {
    pub id: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WireEntry {
    pub id: String,
    pub h: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
}

/// The entries as the page receives them: the html only of those not in
/// `sent` (id -> hash already on the page), which is updated.
pub fn wire(entries: &[Entry], sent: &mut std::collections::HashMap<String, u64>) -> Vec<WireEntry> {
    let mut out = Vec::with_capacity(entries.len());
    let mut kept = std::collections::HashMap::with_capacity(entries.len());
    for e in entries {
        let h = e.hash();
        let known = sent.get(&e.id) == Some(&h);
        out.push(WireEntry { id: e.id.clone(), h: format!("{h:016x}"), html: (!known).then(|| e.html.clone()) });
        kept.insert(e.id.clone(), h);
    }
    *sent = kept;
    out
}

/// The JavaScript call that applies `update` (a JSON literal is valid JavaScript).
pub fn update_script(update: &Update) -> String {
    format!("window.kova && window.kova.update({})", serde_json::to_string(update).unwrap_or_else(|_| "{}".into()))
}

/// The page, loaded once per pane shown in chat mode.
/// The page's colours switched in place (`theme::restyle_script`): the same
/// variables as the `:root` rule of `shell`.
pub fn restyle_script(t: &Theme) -> String {
    let decls = format!(
        "--bg:{};--bg2:{};--t1:{};--t2:{};--t3:{};--b2:{};--b3:{};--accent:{};--working:{};--awaiting:{};--err:{};--ok:{};",
        t.bg, t.bg2, t.t1, t.t2, t.t3, t.b2, t.b3, t.accent, t.working, t.awaiting, t.error, t.ok
    );
    // The ask cards keep the design system's own variables, set on `#asks`.
    let asks = serde_json::to_string(&crate::cockpit::vars(!t.dark)).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        "{}var a=document.getElementById('asks');if(a)a.setAttribute('style',{asks});",
        crate::theme::restyle_script(!t.dark, &decls)
    )
}

pub fn shell(theme: &Theme, title: &str, nonce: &str, terminal: bool) -> String {
    let t = theme;
    // The Cockpit's chat has no terminal to go back to.
    let terminal = if terminal { r#"<button data-act="terminal">Terminal</button>"# } else { "" };
    format!(
        r##"<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src data:; script-src 'nonce-{nonce}'">
<title>{title}</title>
<style>
:root {{ color-scheme: {scheme}; --bg:{bg}; --bg2:{bg2}; --t1:{t1}; --t2:{t2}; --t3:{t3}; --b2:{b2}; --b3:{b3};
  --accent:{accent}; --working:{working}; --awaiting:{awaiting}; --err:{error}; --ok:{ok}; }}
* {{ box-sizing: border-box; }}
html, body {{ margin: 0; height: 100%; background: var(--bg); color: var(--t1);
  font: 14.5px/1.6 "Inter", -apple-system, system-ui, sans-serif; }}
body {{ display: flex; flex-direction: column; }}
#banner {{ display: flex; align-items: center; gap: 8px; min-height: 26px; padding: 0 2px; margin-bottom: 4px;
  font-size: 12.5px; color: var(--t2); }}
#banner #state {{ min-width: 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }}
#banner .dot {{ width: 8px; height: 8px; border-radius: 50%; background: var(--t3); flex: none; }}
#banner.working .dot {{ background: var(--working); animation: pulse 1.2s ease-in-out infinite; }}
#banner.waiting .dot {{ background: var(--awaiting); }}
#banner.waiting {{ color: var(--t1); font-weight: 600; }}
#banner .grow {{ flex: 1; }}
#banner button {{ font: inherit; font-size: 12px; color: var(--t3); background: none; border: 1px solid var(--b3);
  border-radius: 6px; padding: 2px 8px; cursor: pointer; }}
#banner button:hover {{ color: var(--t1); border-color: var(--b2); }}
@keyframes pulse {{ 0%, 100% {{ opacity: .35; }} 50% {{ opacity: 1; }} }}
#scroll {{ flex: 1; overflow-y: auto; padding: 16px 18px 8px; position: relative; }}
#log {{ max-width: 860px; margin: 0 auto; }}
#more {{ display: none; margin: 0 auto 12px; font: inherit; font-size: 12px; color: var(--t3); background: none;
  border: 1px solid var(--b3); border-radius: 6px; padding: 3px 10px; cursor: pointer; }}
#more.on {{ display: block; }}
.user {{ display: flex; justify-content: flex-end; margin: 18px 0 10px; }}
.bubble {{ max-width: 80%; background: var(--bg2); border: 1px solid var(--b3); border-radius: 14px; padding: 7px 12px;
  white-space: pre-wrap; overflow-wrap: anywhere; }}
.asst {{ margin: 8px 0; color: var(--t1); overflow-wrap: anywhere; }}
.asst p {{ margin: .5em 0; }} .asst ul, .asst ol {{ margin: .4em 0; padding-left: 1.4em; }}
.asst h1, .asst h2, .asst h3 {{ font-size: 15px; margin: 1em 0 .4em; }}
.asst a {{ color: var(--accent); text-decoration: none; }} .asst a:hover {{ text-decoration: underline; }}
.asst code {{ font: 12.5px/1.5 "SF Mono", Menlo, monospace; background: var(--bg2); border: 1px solid var(--b3);
  border-radius: 4px; padding: 0 4px; }}
.asst pre {{ background: var(--bg2); border: 1px solid var(--b3); border-radius: 8px; padding: 10px 12px; overflow-x: auto; }}
.asst pre code {{ background: none; border: 0; padding: 0; }}
.asst table {{ border-collapse: collapse; font-size: 13px; }} .asst th, .asst td {{ border: 1px solid var(--b3); padding: 4px 8px; }}
.asst blockquote {{ margin: .6em 0; padding: 0 12px; border-left: 3px solid var(--b2); color: var(--t3); }}
#log > :last-child.asst.streaming::after {{ content: ""; display: inline-block; width: 7px; height: 7px; margin-left: 4px;
  border-radius: 50%; background: var(--working); animation: pulse 1.2s ease-in-out infinite; }}
.note {{ margin: 10px 0; text-align: center; font-size: 12px; color: var(--t3); }}
.tools {{ margin: 4px 0; }}
.trow {{ display: flex; align-items: center; gap: 8px; width: 100%; height: 26px; padding: 0 6px; margin: 0;
  font: inherit; font-size: 13px; color: var(--t2); background: none; border: 0; border-radius: 6px; cursor: pointer; text-align: left; }}
.trow:hover {{ background: var(--bg2); }}
.verb {{ flex: none; color: var(--t1); font-weight: 500; }}
.target {{ flex: 1; min-width: 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis;
  font: 12.5px "SF Mono", Menlo, monospace; color: var(--t3); }}
.tdet {{ flex: none; font-size: 12px; color: var(--t3); }}
.st {{ flex: none; width: 14px; text-align: center; font-size: 12px; }}
.st.ok::before {{ content: "✓"; color: var(--ok); }}
.st.err::before {{ content: "✕"; color: var(--err); }}
.st.stop::before {{ content: "·"; color: var(--t3); }}
.st.run::before {{ content: ""; display: inline-block; width: 7px; height: 7px; border-radius: 50%; background: var(--working);
  animation: pulse 1.2s ease-in-out infinite; }}
.fold {{ display: none; }}
.open > .fold {{ display: block; }}
.group > .fold {{ padding-left: 14px; border-left: 1px solid var(--b3); margin-left: 9px; }}
.tres {{ margin: 2px 0 6px 6px; max-height: 320px; overflow: auto; background: var(--bg2); border: 1px solid var(--b3);
  border-radius: 6px; padding: 8px 10px; font: 12px/1.45 "SF Mono", Menlo, monospace; color: var(--t2); white-space: pre-wrap; overflow-wrap: anywhere; }}
.cut {{ margin: 0 0 6px 10px; font-size: 11.5px; color: var(--t3); }}
#asks {{ flex: none; {ask_vars} line-height: normal; font-family: Inter, "SF Pro Text", -apple-system, system-ui, sans-serif;
  -webkit-font-smoothing: antialiased; }}
{card_css}
#cards {{ max-width: 860px; width: 100%; margin: 0 auto; padding: 0 18px; }}
#cards > .card {{ margin: 8px 0; cursor: auto; }}
#cards .pcard {{ line-height: 1.6; margin: 8px 0; padding: 10px 12px; border: 1px solid var(--b2); border-radius: 10px; background: var(--bg2); }}
.cfrom {{ font-size: 11.5px; color: var(--t3); text-transform: uppercase; letter-spacing: .04em; }}
.cmsg {{ margin: 2px 0 6px; white-space: pre-wrap; overflow-wrap: anywhere; }}
.cdet {{ margin: 4px 0 8px; padding: 6px 8px; background: var(--bg); border: 1px solid var(--b3); border-radius: 6px;
  font: 12px/1.45 "SF Mono", Menlo, monospace; white-space: pre-wrap; overflow-wrap: anywhere; max-height: 160px; overflow: auto; }}
.cfield {{ width: 100%; margin: 2px 0 8px; resize: none; font: inherit; color: var(--t1); background: var(--bg);
  border: 1px solid var(--b3); border-radius: 6px; padding: 5px 8px; }}
.cbtns {{ display: flex; flex-wrap: wrap; gap: 6px; }}
#cards .cbtn {{ font: inherit; font-size: 13px; color: var(--t1); background: var(--bg); border: 1px solid var(--b2);
  border-radius: 6px; padding: 3px 12px; cursor: pointer; }}
#cards .cbtn.primary {{ background: var(--accent); border-color: var(--accent); color: #fff; }}
#cards .cbtn:disabled {{ opacity: .5; cursor: default; }}
.chint {{ margin-top: 6px; font-size: 12px; color: var(--t3); }}
#foot {{ flex: none; max-width: 860px; width: 100%; margin: 0 auto; padding: 6px 18px 12px; }}
#pending {{ max-width: 860px; margin: 0 auto; }}
.pend {{ flex-direction: column; align-items: flex-end; margin: 10px 0; }}
.pend .bubble {{ opacity: .7; border-style: dashed; white-space: pre-wrap; }}
.pend textarea {{ width: 80%; resize: none; font: inherit; color: var(--t1); background: var(--bg2); border: 1px solid var(--accent);
  border-radius: 14px; padding: 7px 12px; outline: none; }}
.pmeta {{ display: flex; gap: 8px; align-items: center; margin-top: 3px; font-size: 12px; color: var(--t3); }}
.pmeta button {{ font: inherit; color: var(--t3); background: none; border: 0; padding: 0; cursor: pointer; }}
.pmeta button:hover {{ color: var(--t1); text-decoration: underline; }}
#inrow {{ display: flex; align-items: flex-end; gap: 6px; }}
.mic {{ flex: none; display: flex; align-items: center; justify-content: center; width: 38px; height: 38px; padding: 0;
  color: var(--t3); background: var(--bg2); border: 1px solid var(--b2); border-radius: 10px; cursor: pointer; }}
.mic:hover {{ color: var(--t1); }}
#mic.on {{ color: #fff; background: var(--err); border-color: var(--err); }}
#hist {{ display: none; }} #hist.on {{ display: flex; }}
#foot {{ position: relative; }}
#pill {{ display: none; position: absolute; left: 50%; bottom: calc(100% + 6px); transform: translateX(-50%); z-index: 5;
  height: 28px; align-items: center; gap: 8px; padding: 0 12px; border-radius: 14px; font-size: 12.5px; white-space: nowrap;
  color: #fff; background: rgba(24, 24, 27, .94); box-shadow: 0 4px 14px rgba(0, 0, 0, .25); }}
#pill.hold, #pill.free, #pill.working, #pill.error, #pill.info {{ display: flex; }}
#pill.error {{ background: var(--err); }}
#pill .bars {{ display: none; align-items: center; gap: 2px; height: 16px; }}
#pill.hold .bars, #pill.free .bars, #pill.working .bars {{ display: flex; }}
#pill .bar {{ width: 3px; height: 16px; border-radius: 2px; background: var(--ok); transform: scaleY(.15); transition: transform .18s ease-out; }}
#pill.working .bar {{ animation: wave 1s ease-in-out infinite; }}
@keyframes wave {{ 0%, 100% {{ transform: scaleY(.2); }} 50% {{ transform: scaleY(.8); }} }}
@media (prefers-reduced-motion: reduce) {{ #pill.working .bar {{ animation: none; transform: scaleY(.5); }} }}
#pill .pbtn {{ display: none; width: 20px; height: 20px; padding: 0; border: 0; border-radius: 10px; cursor: pointer;
  color: #fff; background: rgba(255, 255, 255, .16); font-size: 12px; line-height: 20px; }}
#pill.free .pbtn {{ display: block; }} #pill .pbtn.ok {{ background: var(--ok); }}
#vhist {{ display: none; position: absolute; right: 18px; bottom: calc(100% + 6px); z-index: 6; width: min(420px, 90%);
  max-height: 50vh; overflow: auto; padding: 4px; background: var(--bg2); border: 1px solid var(--b2); border-radius: 10px;
  box-shadow: 0 6px 18px rgba(0, 0, 0, .2); }}
#vhist.on {{ display: block; }}
#vhist .vtitle {{ padding: 4px 8px; font-size: 11.5px; color: var(--t3); }}
#vhist button {{ display: block; width: 100%; text-align: left; font: inherit; font-size: 13px; color: var(--t1); background: none;
  border: 0; border-radius: 6px; padding: 5px 8px; cursor: pointer; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }}
#vhist button:hover {{ background: var(--bg); }}
#box {{ flex: 1; width: 100%; min-height: 38px; max-height: 40vh; resize: none; font: inherit; color: var(--t1); background: var(--bg2);
  border: 1px solid var(--b2); border-radius: 10px; padding: 8px 12px; outline: none; }}
#box:focus {{ border-color: var(--accent); }}
#note {{ min-height: 16px; margin-top: 4px; font-size: 12px; color: var(--t3); }}
.behind {{ margin: 2px 0; }}
.bhead {{ height: 24px; font-size: 12.5px; }} .bhead .verb {{ color: var(--t3); font-weight: 400; }}
.behind > .fold {{ padding-left: 10px; border-left: 1px solid var(--b3); margin: 0 0 6px 9px; font-size: 13px; }}
.asst.bt {{ margin: 4px 6px; font-size: 13px; color: var(--t2); }}
.behind .note {{ text-align: left; margin: 4px 6px; }}
.think {{ margin: 6px 0; font-size: 13px; }} .think > .shimmer {{ padding: 0 6px; }}
.shimmer {{ color: var(--t3); background: linear-gradient(90deg, var(--t3) 35%, var(--t1) 50%, var(--t3) 65%);
  background-size: 250% 100%; -webkit-background-clip: text; background-clip: text; -webkit-text-fill-color: transparent;
  animation: shimmer 2.2s linear infinite; }}
@keyframes shimmer {{ from {{ background-position: 100% 0; }} to {{ background-position: 0 0; }} }}
@media (prefers-reduced-motion: reduce) {{ .shimmer {{ animation: none; -webkit-text-fill-color: var(--t3); }} }}
a.path {{ color: var(--accent); text-decoration: none; }} a.path:hover, .bubble a:hover, .tres a:hover {{ text-decoration: underline; }}
.bubble a, .tres a {{ color: var(--accent); text-decoration: none; }}
#chips {{ display: none; flex-wrap: wrap; gap: 6px; margin-bottom: 6px; }}
#chips.on {{ display: flex; }}
.chip {{ position: relative; display: flex; align-items: center; justify-content: center; width: 56px; height: 56px;
  border: 1px solid var(--b2); border-radius: 8px; background: var(--bg2); font-size: 11px; color: var(--t3); overflow: hidden; }}
.chip img {{ width: 100%; height: 100%; object-fit: cover; }}
.chip button {{ position: absolute; top: 2px; right: 2px; width: 16px; height: 16px; padding: 0; border: 0; border-radius: 50%;
  font-size: 11px; line-height: 16px; color: #fff; background: rgba(0,0,0,.6); cursor: pointer; }}
</style></head><body>
<div id="scroll"><div id="log"><button id="more" data-act="more">Show earlier</button></div><div id="pending"></div></div>
<div id="asks"><div id="cards"></div>{snooze_html}</div>
<div id="foot"><div id="chips"></div><div id="banner"><span class="dot"></span><span id="state">…</span><span class="grow"></span>{terminal}</div>
<div id="pill"><span class="bars"><span class="bar"></span><span class="bar"></span><span class="bar"></span><span class="bar"></span><span class="bar"></span><span class="bar"></span><span class="bar"></span><span class="bar"></span><span class="bar"></span></span><span id="pline"></span><button class="pbtn" data-act="vcancel" title="Annuler">✕</button><button class="pbtn ok" data-act="vfinish" title="Terminer">✓</button></div>
<div id="vhist"></div>
<div id="inrow"><textarea id="box" rows="1" placeholder="Message à Claude"></textarea><button id="hist" class="mic" data-act="vhist" title="Dictées récentes"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg></button><button id="mic" class="mic" title="Dictée : maintenir ⌘⇧M pour parler, toucher pour continuer"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="2" width="6" height="12" rx="3"/><path d="M5 10a7 7 0 0 0 14 0M12 17v5"/></svg></button></div><div id="note"></div></div>
<script nonce="{nonce}">
(function () {{
  // Each message carries this load's nonce: Kova drops one from another load.
  const post = (m) => {{ try {{ m.load = "{nonce}"; window.webkit.messageHandlers.{handler}.postMessage(JSON.stringify(m)); }} catch (e) {{}} }};
  const $ = (id) => document.getElementById(id);
  const log = $("log"), scroll = $("scroll"), box = $("box"), cardsEl = $("cards");
  const opened = new Set();
  let noteTimer = null;
  // Voice typing: the pill Kova drives, the clean text inserted at the
  // cursor, the last dictations to put back.
  const pill = $("pill"), bars = Array.from(pill.querySelectorAll(".bar"));
  const shape = [.45, .7, .9, 1, .8, 1, .9, .7, .45];
  let vHist = [];
  function applyVoice(v) {{
    pill.className = v.state;
    const on = v.state === "hold" || v.state === "free";
    $("mic").classList.toggle("on", on);
    $("pline").textContent = v.line;
    bars.forEach((b, i) => {{ b.style.transform = on ? "scaleY(" + Math.max(.15, shape[i] * v.level / 10) + ")" : ""; }});
    vHist = v.history;
    $("hist").classList.toggle("on", vHist.length > 0);
    if ($("vhist").classList.contains("on")) histList();
  }}
  function insertAt(text) {{
    // Never focused, its cursor is at 0: then the text goes at the end.
    const at = document.activeElement === box;
    const s = at ? box.selectionStart : box.value.length, e = at ? box.selectionEnd : s;
    const before = box.value.slice(0, s), after = box.value.slice(e);
    const lead = before && !/\s$/.test(before) ? " " : "";
    const tail = after && !/^\s/.test(after) ? " " : "";
    box.setRangeText(lead + text + tail, s, e, "end");
    box.focus(); grow();
  }}
  function histList() {{
    const el = $("vhist");
    el.textContent = "";
    const title = document.createElement("div");
    title.className = "vtitle"; title.textContent = "Dictées récentes";
    el.appendChild(title);
    vHist.forEach((t) => {{
      const b = document.createElement("button");
      b.textContent = t; b.title = t;
      b.addEventListener("click", () => {{ el.classList.remove("on"); insertAt(t); }});
      el.appendChild(b);
    }});
  }}
  // Hold the button to talk, tap it to keep listening: Kova tells them apart.
  const mic = $("mic");
  mic.addEventListener("pointerdown", (e) => {{ e.preventDefault(); mic.setPointerCapture(e.pointerId); post({{ type: "voice-press" }}); }});
  mic.addEventListener("pointerup", () => post({{ type: "voice-release" }}));
  mic.addEventListener("pointercancel", () => post({{ type: "voice-release" }}));
  // Images pasted into the box, sent with the next message: their paths.
  let images = [];
  function chips() {{
    const el = $("chips");
    el.replaceChildren();
    images.forEach((im, i) => {{
      const c = document.createElement("span");
      c.className = "chip"; c.title = im.path;
      if (im.src) {{ const img = document.createElement("img"); img.src = im.src; c.appendChild(img); }}
      else c.appendChild(document.createTextNode("image"));
      const x = document.createElement("button");
      x.textContent = "×"; x.dataset.act = "unchip"; x.dataset.i = String(i);
      c.appendChild(x); el.appendChild(c);
    }});
    el.classList.toggle("on", images.length > 0);
  }}
  function nodeFrom(html) {{
    const t = document.createElement("template");
    t.innerHTML = html;
    return t.content.firstElementChild;
  }}
  function reopen(node) {{
    if (node.dataset.id && opened.has(node.dataset.id)) node.classList.add("open");
    node.querySelectorAll("[data-id]").forEach((n) => {{ if (opened.has(n.dataset.id)) n.classList.add("open"); }});
  }}
  function reconcile(parent, list, anchor) {{
    const have = new Map();
    for (const n of Array.from(parent.children)) if (n.dataset.key) have.set(n.dataset.key, n);
    let missing = false, prev = anchor;
    const keep = new Set();
    for (const e of list) {{
      let n = have.get(e.id);
      // Html sent again for the same hash is a card to enable again.
      if (!n || n.dataset.h !== e.h || e.html !== undefined) {{
        if (e.html === undefined) {{ missing = true; if (!n) continue; }}
        else {{
          const fresh = nodeFrom(e.html);
          if (!fresh) continue;
          fresh.dataset.key = e.id; fresh.dataset.h = e.h;
          if (n) {{
            // A card keeps what was typed in its fields, and the one typed in.
            const olds = n.querySelectorAll("textarea"), neus = fresh.querySelectorAll("textarea");
            const a = document.activeElement;
            let focus = null;
            neus.forEach((t, i) => {{
              const o = olds[i];
              if (!o || o.className !== t.className) return;
              t.value = o.value;
              if (o === a) focus = [t, o.selectionStart, o.selectionEnd];
            }});
            n.replaceWith(fresh);
            if (focus) {{ focus[0].focus(); focus[0].setSelectionRange(focus[1], focus[2]); }}
          }}
          n = fresh; reopen(n);
        }}
      }}
      keep.add(e.id);
      const next = prev ? prev.nextSibling : parent.firstChild;
      if (n !== next) parent.insertBefore(n, next);
      prev = n;
    }}
    for (const [k, n] of have) if (!keep.has(k)) n.remove();
    return missing;
  }}
  function update(m) {{
    if (m.banner) {{ $("state").textContent = m.banner.text; $("banner").className = m.banner.state; }}
    const atBottom = scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight < 48;
    let missing = false;
    if (m.entries) {{
      missing = reconcile(log, m.entries, $("more")) || missing;
      $("more").classList.toggle("on", !!m.more);
    }}
    if (m.cards) missing = reconcile(cardsEl, m.cards, null) || missing;
    const last = log.lastElementChild;
    log.querySelectorAll(".streaming").forEach((n) => {{ if (n !== last || !m.working) n.classList.remove("streaming"); }});
    if (last && m.working && last.classList.contains("asst")) last.classList.add("streaming");
    if (m.banner) {{
      box.placeholder = m.banner.state === "done" ? "Message à Claude" : "Message à Claude, il partira à son tour";
    }}
    if (m.queued) pending(m.queued);
    if (m.attach) {{ images.push(...m.attach); chips(); }}
    if (m.voice) applyVoice(m.voice);
    if (m.dictated) {{ insertAt(m.dictated.text); if (m.dictated.send) send(); }}
    if (m.restore) {{ box.value = box.value ? box.value + "\n" + m.restore : m.restore; grow(); }}
    if (m.note) {{
      $("note").textContent = m.note;
      clearTimeout(noteTimer);
      noteTimer = setTimeout(() => {{ $("note").textContent = ""; }}, 6000);
    }}
    if (m.first) {{
      const users = log.querySelectorAll(".user");
      const u = users[users.length - 1];
      if (u && log.scrollHeight - u.offsetTop > scroll.clientHeight) scroll.scrollTop = u.offsetTop - 12;
      else scroll.scrollTop = scroll.scrollHeight;
    }} else if (atBottom && (m.entries || m.cards || m.queued)) {{
      scroll.scrollTop = scroll.scrollHeight;
    }}
    if (m.focus) box.focus();
    if (missing) post({{ type: "resync" }});
  }}
  // The queue as "en attente" bubbles, editable and cancellable until they
  // leave. One being edited keeps its field.
  let editing = null;
  function pending(list) {{
    const el = $("pending");
    if (editing !== null && !list.some((p) => p.id === editing)) editing = null;
    el.querySelectorAll(".pend").forEach((n) => {{ if (Number(n.dataset.id) !== editing) n.remove(); }});
    for (const p of list) {{
      let n = el.querySelector(`.pend[data-id="${{p.id}}"]`);
      if (n) {{ el.appendChild(n); continue; }}
      n = document.createElement("div");
      n.className = "user pend"; n.dataset.id = p.id;
      const b = document.createElement("div"); b.className = "bubble"; b.textContent = p.text;
      const meta = document.createElement("div"); meta.className = "pmeta";
      meta.innerHTML = '<span>En attente</span><button data-act="pedit">Modifier</button><button data-act="pcancel">Annuler</button>';
      n.append(b, meta);
      el.appendChild(n);
    }}
  }}
  function editPending(n) {{
    const b = n.querySelector(".bubble");
    if (!b) return;
    editing = Number(n.dataset.id);
    const f = document.createElement("textarea");
    f.value = b.textContent; f.rows = Math.min(8, f.value.split("\n").length + 1);
    b.replaceWith(f);
    n.querySelector(".pmeta").innerHTML = '<button data-act="psave">Enregistrer</button><button data-act="pcancel">Annuler le message</button>';
    f.focus();
    f.addEventListener("keydown", (e) => {{
      if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {{ e.preventDefault(); savePending(n); }}
    }});
  }}
  function savePending(n) {{
    const f = n.querySelector("textarea");
    const id = Number(n.dataset.id);
    editing = null;
    n.remove();
    post({{ type: "edit", id, text: f ? f.value : "" }});
  }}
  function grow() {{ box.style.height = "auto"; box.style.height = Math.min(box.scrollHeight + 2, window.innerHeight * 0.4) + "px"; }}
  box.addEventListener("input", grow);
  function send() {{
    const text = box.value;
    if (!text.trim() && !images.length) return;
    const paths = images.map((im) => im.path);
    box.value = ""; grow(); images = []; chips();
    post({{ type: "send", text, images: paths }});
  }}
  box.addEventListener("keydown", (e) => {{
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {{
      e.preventDefault();
      send();
    }} else if (e.key === "Escape" && $("vhist").classList.contains("on")) {{
      $("vhist").classList.remove("on");
    }}
  }});
  // An image on the pasteboard (Cmd+V): Kova writes it to a file, the box
  // shows it, its path goes with the message.
  box.addEventListener("paste", (e) => {{
    const items = e.clipboardData ? Array.from(e.clipboardData.items) : [];
    if (items.some((i) => i.kind === "file" && i.type.startsWith("image/"))) {{
      e.preventDefault();
      post({{ type: "paste-image" }});
    }}
  }});
  // An ask card's icons, as on the Cockpit: the snooze menu, remove, done.
  {snooze_js}
  let snzKey = null;
  function snzClose() {{ $("snz").hidden = true; $("snzPick").hidden = true; snzKey = null; }}
  function hideCard(key) {{ const c = cardsEl.querySelector('.card[data-key="' + CSS.escape(key) + '"]'); if (c) c.style.display = "none"; }}
  function snzClick(e) {{
    const b = e.target.closest("button");
    if (!b || !snzKey) return;
    if (b.dataset.when === "pick") {{ $("snzPick").hidden = false; $("snzAt").focus(); return; }}
    const d = when(b.dataset.when);
    if (!d || isNaN(d) || d <= new Date()) return;
    const id = snzKey;
    snzClose(); hideCard(id);
    post({{ type: "ask", id, act: "snooze", text: iso(d) }});
  }}
  function snzOpen(b, key) {{
    snzKey = key;
    const r = b.getBoundingClientRect(), m = $("snz");
    m.hidden = false;
    m.style.top = Math.max(8, r.top - m.offsetHeight - 4) + "px";
    m.style.left = Math.max(8, Math.min(r.right - m.offsetWidth, window.innerWidth - m.offsetWidth - 8)) + "px";
    $("snzAt").value = iso(new Date(Date.now() + 3600e3)).slice(0, 16);
  }}
  // An instruction on an ask card: Enter sends it, Shift+Enter is a new line.
  cardsEl.addEventListener("keydown", (e) => {{
    const t = e.target;
    if (e.key !== "Enter" || e.shiftKey || e.isComposing || !t.classList.contains("ins")) return;
    e.preventDefault();
    if (t.readOnly || !t.value.trim()) return;
    post({{ type: "ask", id: t.closest(".card").dataset.key, act: "instruct", text: t.value }});
    t.value = "";
  }});
  document.addEventListener("keydown", (e) => {{ if (e.key === "Escape" && !$("snz").hidden) snzClose(); }});
  document.addEventListener("click", (e) => {{
    if (e.target.closest("#snz")) return snzClick(e);
    if (!$("snz").hidden) snzClose();
    if (!e.target.closest("#vhist, #hist")) $("vhist").classList.remove("on");
    const a = e.target.closest("a");
    if (a) {{
      // Every link opens in Kova's side panel, never in this page.
      e.preventDefault();
      const href = a.dataset.path || a.getAttribute("href");
      if (href && href !== "#") post({{ type: "link", href }});
      return;
    }}
    const b = e.target.closest("[data-act]");
    if (!b) return;
    const act = b.dataset.act;
    if (act === "unchip") {{ images.splice(Number(b.dataset.i), 1); chips(); return; }}
    if (act === "fold") {{
      const holder = b.parentElement;
      holder.classList.toggle("open");
      if (holder.classList.contains("open")) opened.add(holder.dataset.id); else opened.delete(holder.dataset.id);
      return;
    }}
    const card = b.closest(".card, .pcard");
    if (act === "answer" && card) {{
      const field = card.querySelector("textarea.cfield, textarea.ans");
      const text = field ? field.value : "";
      if ((b.dataset.needsText || b.dataset.text) && !text.trim()) {{ if (field) field.focus(); return; }}
      card.querySelectorAll("button").forEach((x) => x.disabled = true);
      post({{ type: "answer", id: card.dataset.key, action: b.dataset.action || b.dataset.arg, text }});
    }} else if (act === "permission" && card) {{
      card.querySelectorAll("button").forEach((x) => x.disabled = true);
      post({{ type: "permission", id: card.dataset.key, choice: b.dataset.choice }});
    }} else if (act === "open" && card) {{
      post({{ type: "open", id: card.dataset.key }});
    }} else if (act === "snooze" && card) {{
      snzOpen(b, card.dataset.key);
    }} else if (act === "link" && b.dataset.arg) {{
      post({{ type: "link", href: b.dataset.arg }});
    }} else if ((act === "dismiss" || act === "done") && card) {{
      hideCard(card.dataset.key);
      post({{ type: "ask", id: card.dataset.key, act }});
    }} else if (act === "vcancel" || act === "vfinish") {{
      post({{ type: act === "vcancel" ? "voice-cancel" : "voice-finish" }});
    }} else if (act === "vhist") {{
      if ($("vhist").classList.toggle("on")) histList();
    }} else if (act === "pedit" || act === "psave" || act === "pcancel") {{
      const n = b.closest(".pend");
      if (!n) return;
      if (act === "pedit") editPending(n);
      else if (act === "psave") savePending(n);
      else {{ if (editing === Number(n.dataset.id)) editing = null; n.remove(); post({{ type: "cancel", id: Number(n.dataset.id) }}); }}
    }} else if (act === "more" || act === "terminal") {{
      post({{ type: act }});
    }}
  }});
  window.kova = {{ update }};
  post({{ type: "ready" }});
}})();
</script></body></html>"##,
        nonce = nonce,
        terminal = terminal,
        handler = HANDLER,
        ask_vars = crate::cockpit::vars(!t.dark),
        card_css = crate::cockpit::scoped_card_css("#asks"),
        snooze_html = crate::cockpit::SNOOZE_HTML,
        snooze_js = crate::cockpit::SNOOZE_JS,
        title = escape(title),
        scheme = if t.dark { "dark" } else { "light" },
        bg = t.bg,
        bg2 = t.bg2,
        t1 = t.t1,
        t2 = t.t2,
        t3 = t.t3,
        b2 = t.b2,
        b3 = t.b3,
        accent = t.accent,
        working = t.working,
        awaiting = t.awaiting,
        error = t.error,
        ok = t.ok,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::tail::RawLine;
    use serde_json::json;

    fn conv(lines: Vec<serde_json::Value>) -> Conversation {
        let mut c = Conversation::new();
        for (i, value) in lines.into_iter().enumerate() {
            c.apply(&RawLine { offset: i as u64, value });
        }
        c
    }

    fn user(id: &str, text: &str) -> serde_json::Value {
        json!({"type": "user", "uuid": id, "message": {"content": text}})
    }

    fn tool(req: &str, i: i64, id: &str) -> serde_json::Value {
        json!({"type": "assistant", "requestId": req, "apiBlockIndex": i, "uuid": format!("{req}{i}"),
               "message": {"content": [{"type": "tool_use", "id": id, "name": "Bash", "input": {"command": format!("echo {id}")}}]}})
    }

    fn text(req: &str, i: i64, t: &str) -> serde_json::Value {
        json!({"type": "assistant", "requestId": req, "apiBlockIndex": i, "uuid": format!("{req}{i}"),
               "message": {"content": [{"type": "text", "text": t}]}})
    }

    fn result(id: &str, uuid: &str, err: bool) -> serde_json::Value {
        json!({"type": "user", "uuid": uuid, "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": "out", "is_error": err}]}})
    }

    fn ids(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.id.as_str()).collect()
    }

    #[test]
    fn only_the_user_s_messages_and_the_final_text_show_the_rest_folds() {
        let c = conv(vec![
            user("u1", "go"),
            text("r0", 0, "Let me look."),
            tool("r1", 0, "a"),
            json!({"type": "assistant", "requestId": "r2", "uuid": "r2t", "message": {"content": [{"type": "thinking", "thinking": ""}]}}),
            tool("r2", 1, "b"),
            text("r3", 0, "Done."),
            json!({"type": "attachment", "uuid": "q", "attachment": {"type": "queued_command", "commandMode": "prompt", "origin": {"kind": "peer"},
                   "prompt": "<cross-session-message from-name=\"pm\">hi</cross-session-message>"}}),
            user("u2", "<system-reminder>x</system-reminder>"),
            user("u3", "and now?"),
            json!({"type": "user", "uuid": "n", "message": {"content": "[Request interrupted by user]"}}),
        ]);
        let (e, more) = entries(&c, EXCHANGES_STEP, false);
        assert!(!more);
        assert_eq!(ids(&e), ["u:u1", "c:t:r0:0", "t:r3:0", "c:b:q", "u:u3", "c:n:n"]);
        assert!(e[1].html.contains("3 actions en coulisse"), "a text before a call, two calls; an empty thinking is nothing: {}", e[1].html);
        assert_eq!(e[1].html.matches(r#"class="tool""#).count(), 2);
        assert!(e[1].html.contains(r#"<div class="fold">"#), "folded by default, unfolded on a click");
        assert!(e[3].html.contains("1 action en coulisse") && e[3].html.contains("Message de pm"));
        assert!(e[5].html.contains("Interrupted"));
    }

    #[test]
    fn the_last_text_shows_while_the_turn_goes_on_and_folds_once_a_call_follows() {
        let mut lines = vec![user("u1", "go"), text("r1", 0, "Looking.")];
        let (e, _) = entries(&conv(lines.clone()), EXCHANGES_STEP, true);
        assert_eq!(ids(&e), ["u:u1", "t:r1:0", "r:live"], "the model thinks: the Réflexion line ends the thread");
        lines.push(tool("r2", 0, "a"));
        let (e, _) = entries(&conv(lines), EXCHANGES_STEP, true);
        assert_eq!(ids(&e), ["u:u1", "c:t:r1:0"], "a call runs: no Réflexion line");
        assert!(e[1].html.contains(r#"class="st run""#), "the running call shows on the folded line");
    }

    #[test]
    fn reflexion_unfolds_its_text_only_when_the_transcript_has_it() {
        let think = |t: &str| json!({"type": "assistant", "requestId": "k", "uuid": format!("k{t}"), "message": {"content": [{"type": "thinking", "thinking": t}]}});
        let (e, _) = entries(&conv(vec![user("u1", "go"), think("")]), EXCHANGES_STEP, true);
        let last = e.last().unwrap();
        assert!(last.html.contains("shimmer") && !last.html.contains("data-act"), "nothing to unfold");
        let (e, _) = entries(&conv(vec![user("u1", "go"), think("weigh <both>")]), EXCHANGES_STEP, true);
        let last = e.last().unwrap();
        assert!(last.html.contains(r#"data-act="fold""#) && last.html.contains("weigh &lt;both&gt;"));
        assert_eq!(ids(&e), ["u:u1", "r:live"], "the reasoning going on is not also behind the scenes");
        let (e, _) = entries(&conv(vec![user("u1", "go"), think("x")]), EXCHANGES_STEP, false);
        assert!(!e.iter().any(|x| x.id == "r:live"), "idle: no Réflexion");
    }

    #[test]
    fn links_open_in_the_panel_but_not_inside_code() {
        let html = markdown("See https://github.com/a/b/pull/1 and /tmp/r.md, `/tmp/c.md` and `https://x.io`\n\n```\nhttps://y.io\n```\n[plan](~/p/plan.md) [s](/etc/hosts)");
        assert!(html.contains(r#"<a href="https://github.com/a/b/pull/1">"#));
        assert!(html.contains(r#"data-path="/tmp/r.md""#));
        assert!(html.contains(r#"<code>/tmp/c.md</code>"#) && html.contains(r#"<code>https://x.io</code>"#), "{html}");
        assert!(!html.contains(r#"href="https://y.io""#), "a code block stays text");
        assert!(html.contains(r#"data-path="~/p/plan.md">plan</a>"#));
        assert!(!html.contains("/etc/hosts"), "a link to another file keeps its text only");
        let c = conv(vec![user("u1", "open https://linear.app/x/issue/CLA-1")]);
        assert!(entries(&c, EXCHANGES_STEP, false).0[0].html.contains(r#"<a href="https://linear.app/x/issue/CLA-1">"#));
    }

    #[test]
    fn only_the_current_turn_has_a_running_call() {
        let c = conv(vec![user("u1", "go"), tool("r1", 0, "old"), user("u2", "next"), tool("r2", 0, "new")]);
        let (e, _) = entries(&c, EXCHANGES_STEP, true);
        assert!(e[1].html.contains(r#"class="st stop""#), "an earlier turn's call without result was stopped");
        assert!(e[3].html.contains(r#"class="st run""#));
    }

    #[test]
    fn a_run_past_five_folds_into_n_actions() {
        let mut lines = vec![user("u1", "go")];
        for i in 0..6 {
            lines.push(tool(&format!("r{i}"), 0, &format!("t{i}")));
            if i < 5 {
                lines.push(result(&format!("t{i}"), &format!("res{i}"), i == 2));
            }
        }
        let (e, _) = entries(&conv(lines.clone()), EXCHANGES_STEP, true);
        assert!(e[1].html.contains("6 actions"));
        assert!(e[1].html.contains(r#"<span class="st run">"#), "the call still running shows on the group");
        let (e, _) = entries(&conv(lines), EXCHANGES_STEP, false);
        assert!(e[1].html.contains("ghead"));
        assert!(e[1].html.contains(r#"class="st err""#) && e[1].html.contains(r#"class="st stop""#));
    }

    #[test]
    fn the_page_opens_on_the_last_exchanges() {
        let c = conv(vec![user("u1", "one"), text("a", 0, "1"), user("u2", "two"), text("b", 0, "2"), user("u3", "three")]);
        let (e, more) = entries(&c, 2, false);
        assert!(more);
        assert_eq!(ids(&e), ["u:u2", "t:b:0", "u:u3"]);
    }

    #[test]
    fn transcript_text_is_data_never_markup() {
        let c = conv(vec![
            user("u1", "<img src=x onerror=alert(1)>"),
            text("a", 0, "<script>alert(1)</script>\n\n[x](javascript:alert(1)) ![i](https://tracker/p.png) [ok](https://a.b)"),
        ]);
        let (e, _) = entries(&c, EXCHANGES_STEP, false);
        assert!(e[0].html.contains("&lt;img src=x onerror=alert(1)&gt;"));
        assert!(!e[1].html.contains("<script>"));
        assert!(e[1].html.contains("&lt;script&gt;"), "raw HTML is shown as text");
        assert!(!e[1].html.contains("javascript:"));
        assert!(e[1].html.contains("<p>x "), "a link to another scheme keeps its text, not the link");
        assert!(!e[1].html.contains(r#"href="""#), "no empty link: clicking one would blank the page");
        assert!(!e[1].html.contains("tracker"), "no remote image is ever fetched");
        assert!(e[1].html.contains(r#"href="https://a.b""#));
    }

    #[test]
    fn only_what_changed_goes_over_the_wire() {
        let mut sent = std::collections::HashMap::new();
        let a = Entry { id: "a".into(), html: "<p>a</p>".into() };
        let b = Entry { id: "b".into(), html: "<p>b</p>".into() };
        let first = wire(&[a.clone(), b.clone()], &mut sent);
        assert!(first.iter().all(|w| w.html.is_some()));
        let b2 = Entry { id: "b".into(), html: "<p>b, more</p>".into() };
        let second = wire(&[a, b2], &mut sent);
        assert_eq!(second[0].html, None);
        assert_eq!(second[1].html.as_deref(), Some("<p>b, more</p>"));
    }

    #[test]
    fn the_page_runs_only_its_own_script() {
        let theme = Theme {
            dark: true,
            bg: "#000".into(),
            bg2: "#111".into(),
            t1: "#fff".into(),
            t2: "#eee".into(),
            t3: "#ddd".into(),
            b2: "#333".into(),
            b3: "#222".into(),
            accent: "#36f".into(),
            working: "#3c6".into(),
            awaiting: "#fa0".into(),
            error: "#f33".into(),
            ok: "#3c6".into(),
        };
        let page = shell(&theme, "<x>", "abc123", true);
        assert!(page.contains("script-src 'nonce-abc123'"));
        assert_eq!(page.matches("<script").count(), 1);
        assert!(page.contains(r#"<script nonce="abc123">"#));
        assert!(!page.contains("onclick"), "no inline handler: the policy would refuse it anyway");
        assert!(page.contains("<title>&lt;x&gt;</title>"));
        // The pane's chat goes back to its terminal; the Cockpit's has none.
        assert!(page.contains(r#"data-act="terminal""#));
        assert!(!shell(&theme, "<x>", "abc123", false).contains(r#"data-act="terminal""#));
        // The ask cards: the Cockpit's look on the design system's dark
        // tokens, its snooze menu, never the old orange bar.
        let dark = crate::cockpit::vars(false);
        assert!(page.contains(&format!("#asks {{ flex: none; {dark}")), "the tokens are scoped to the cards");
        assert!(page.contains("#asks .card .t{font-size:15px") && page.contains(r#"<div class="snz" id="snz" hidden>"#));
        assert!(page.contains("function iso(d)") && !page.contains("border-left: 3px solid var(--awaiting)"));
        assert!(restyle_script(&Theme { dark: false, ..theme }).contains(&crate::cockpit::vars(true)[..20]));
    }
}
