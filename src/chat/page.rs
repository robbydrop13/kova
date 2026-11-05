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
//! (`link/docs/13-chat-lisibilite.md`): a state banner at the top, the
//! assistant's text full width without a bubble, the user's message in a
//! quiet bubble on the right, tool calls as compact one-line rows grouped
//! and folded past five, the last exchanges first, nothing folding by
//! itself.

use serde::Serialize;

use super::conversation::{BlockKind, Conversation, Item, Tool};

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

/// Markdown as an HTML fragment: raw HTML as text, a link only to http(s)
/// or mailto, no image source.
pub fn markdown(source: &str) -> String {
    use pulldown_cmark::{html, Event, Options, Parser, Tag};
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let parser = Parser::new_ext(source, options).map(|event| match event {
        Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
        Event::Start(Tag::Link { link_type, dest_url, title, id }) => {
            let lower = dest_url.trim().to_ascii_lowercase();
            let safe = lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:");
            Event::Start(Tag::Link { link_type, dest_url: if safe { dest_url } else { "".into() }, title, id })
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

/// The entries of the last `exchanges` exchanges, and whether earlier ones
/// exist. `working` says whether a tool call without its result is running
/// (or was stopped).
pub fn entries(conv: &Conversation, exchanges: usize, working: bool) -> (Vec<Entry>, bool) {
    let users: Vec<usize> = conv.items.iter().enumerate().filter(|(_, i)| matches!(i, Item::User { .. })).map(|(n, _)| n).collect();
    let start = if users.len() > exchanges { users[users.len() - exchanges] } else { 0 };
    let mut out = Vec::new();
    let mut group: Vec<&Tool> = Vec::new();
    let flush = |group: &mut Vec<&Tool>, out: &mut Vec<Entry>| {
        if !group.is_empty() {
            out.push(tool_group(group, working));
            group.clear();
        }
    };
    for item in &conv.items[start..] {
        match item {
            Item::User { id, text, .. } => {
                flush(&mut group, &mut out);
                out.push(Entry {
                    id: format!("u:{id}"),
                    html: format!(r#"<div class="user"><div class="bubble">{}</div></div>"#, escape(text)),
                });
            }
            Item::Note { id, text } => {
                flush(&mut group, &mut out);
                out.push(Entry { id: format!("n:{id}"), html: format!(r#"<div class="note">{}</div>"#, escape(text)) });
            }
            Item::Assistant { id, blocks, .. } => {
                for (k, block) in blocks.iter().enumerate() {
                    match &block.kind {
                        BlockKind::Text(text) => {
                            flush(&mut group, &mut out);
                            out.push(Entry { id: format!("t:{id}:{k}"), html: format!(r#"<div class="asst">{}</div>"#, markdown(text)) });
                        }
                        BlockKind::Tool(tool) => group.push(tool),
                        BlockKind::Image => {
                            flush(&mut group, &mut out);
                            out.push(Entry { id: format!("i:{id}:{k}"), html: r#"<div class="note">image</div>"#.to_string() });
                        }
                        // A thinking block has no text worth showing (its
                        // content is empty in the transcript), and it must
                        // not split a run of tool calls.
                        BlockKind::Thinking => {}
                    }
                }
            }
        }
    }
    flush(&mut group, &mut out);
    (out, start > 0)
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

fn tool_group(tools: &[&Tool], working: bool) -> Entry {
    let id = format!("g:{}", tools[0].id);
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

/// A card's HTML: its message, its detail, its field, its buttons.
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
            let class = if i == 0 { "btn primary" } else { "btn" };
            let needs = if b.needs_text { r#" data-needs-text="1""# } else { "" };
            format!(r#"<button class="{class}" data-act="{act}"{extra}{needs}>{}</button>"#, escape(&b.label))
        })
        .collect();
    let hint = card.hint.as_deref().map(|h| format!(r#"<div class="chint">{}</div>"#, escape(h))).unwrap_or_default();
    let kind = serde_json::to_value(&card.kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    format!(
        r#"<div class="card {kind}" data-id="{id}"><div class="cfrom">{from}</div><div class="cmsg">{msg}</div>{detail}{field}<div class="cbtns">{buttons}</div>{hint}</div>"#,
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
    /// The message waiting in the queue, and why.
    pub queued: Option<Queued>,
    /// A one-off line under the box.
    pub note: Option<String>,
    /// Text to put back in the box (a message that was not sent).
    pub restore: Option<String>,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Queued {
    pub text: String,
    pub reason: String,
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
pub fn shell(theme: &Theme, title: &str, nonce: &str) -> String {
    let t = theme;
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
#banner {{ flex: none; display: flex; align-items: center; gap: 8px; height: 34px; padding: 0 12px;
  border-bottom: 1px solid var(--b3); font-size: 13px; color: var(--t2); background: var(--bg); }}
#banner .dot {{ width: 8px; height: 8px; border-radius: 50%; background: var(--t3); flex: none; }}
#banner.working .dot {{ background: var(--working); animation: pulse 1.2s ease-in-out infinite; }}
#banner.waiting .dot {{ background: var(--awaiting); }}
#banner.waiting {{ color: var(--t1); font-weight: 600; }}
#banner .grow {{ flex: 1; }}
#banner button {{ font: inherit; font-size: 12px; color: var(--t3); background: none; border: 1px solid var(--b3);
  border-radius: 6px; padding: 2px 8px; cursor: pointer; }}
#banner button:hover {{ color: var(--t1); border-color: var(--b2); }}
@keyframes pulse {{ 0%, 100% {{ opacity: .35; }} 50% {{ opacity: 1; }} }}
#scroll {{ flex: 1; overflow-y: auto; padding: 16px 18px 8px; }}
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
#cards {{ flex: none; max-width: 860px; width: 100%; margin: 0 auto; padding: 0 18px; }}
.card {{ margin: 8px 0; padding: 10px 12px; border: 1px solid var(--b2); border-left: 3px solid var(--awaiting);
  border-radius: 10px; background: var(--bg2); }}
.cfrom {{ font-size: 11.5px; color: var(--t3); text-transform: uppercase; letter-spacing: .04em; }}
.cmsg {{ margin: 2px 0 6px; white-space: pre-wrap; overflow-wrap: anywhere; }}
.cdet {{ margin: 4px 0 8px; padding: 6px 8px; background: var(--bg); border: 1px solid var(--b3); border-radius: 6px;
  font: 12px/1.45 "SF Mono", Menlo, monospace; white-space: pre-wrap; overflow-wrap: anywhere; max-height: 160px; overflow: auto; }}
.cfield {{ width: 100%; margin: 2px 0 8px; resize: none; font: inherit; color: var(--t1); background: var(--bg);
  border: 1px solid var(--b3); border-radius: 6px; padding: 5px 8px; }}
.cbtns {{ display: flex; flex-wrap: wrap; gap: 6px; }}
.btn {{ font: inherit; font-size: 13px; color: var(--t1); background: var(--bg); border: 1px solid var(--b2);
  border-radius: 6px; padding: 3px 12px; cursor: pointer; }}
.btn.primary {{ background: var(--accent); border-color: var(--accent); color: #fff; }}
.btn:disabled {{ opacity: .5; cursor: default; }}
.chint {{ margin-top: 6px; font-size: 12px; color: var(--t3); }}
#foot {{ flex: none; max-width: 860px; width: 100%; margin: 0 auto; padding: 6px 18px 12px; }}
#queued {{ display: none; align-items: center; gap: 8px; margin-bottom: 6px; font-size: 12.5px; color: var(--t2); }}
#queued.on {{ display: flex; }}
#queued .qtext {{ flex: 1; min-width: 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }}
#box {{ width: 100%; min-height: 38px; max-height: 40vh; resize: none; font: inherit; color: var(--t1); background: var(--bg2);
  border: 1px solid var(--b2); border-radius: 10px; padding: 8px 12px; outline: none; }}
#box:focus {{ border-color: var(--accent); }}
#note {{ min-height: 16px; margin-top: 4px; font-size: 12px; color: var(--t3); }}
</style></head><body>
<div id="banner"><span class="dot"></span><span id="state">…</span><span class="grow"></span><button data-act="terminal">Terminal</button></div>
<div id="scroll"><div id="log"><button id="more" data-act="more">Show earlier</button></div></div>
<div id="cards"></div>
<div id="foot"><div id="queued"><span>Queued:</span><span class="qtext"></span><span class="qwhy"></span><button class="btn" data-act="cancel">Cancel</button></div>
<textarea id="box" rows="1" placeholder="Message Claude"></textarea><div id="note"></div></div>
<script nonce="{nonce}">
(function () {{
  const post = (m) => {{ try {{ window.webkit.messageHandlers.{handler}.postMessage(JSON.stringify(m)); }} catch (e) {{}} }};
  const $ = (id) => document.getElementById(id);
  const log = $("log"), scroll = $("scroll"), box = $("box"), cardsEl = $("cards");
  const opened = new Set();
  let noteTimer = null;
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
      if (!n || n.dataset.h !== e.h) {{
        if (e.html === undefined) {{ missing = true; if (!n) continue; }}
        else {{
          const fresh = nodeFrom(e.html);
          if (!fresh) continue;
          fresh.dataset.key = e.id; fresh.dataset.h = e.h;
          if (n) {{
            // A card keeps what was typed in its field.
            const old = n.querySelector("textarea"), neu = fresh.querySelector("textarea");
            if (old && neu) neu.value = old.value;
            n.replaceWith(fresh);
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
    const q = $("queued");
    q.classList.toggle("on", !!m.queued);
    if (m.queued) {{ q.querySelector(".qtext").textContent = m.queued.text; q.querySelector(".qwhy").textContent = m.queued.reason; }}
    if (m.restore && !box.value) {{ box.value = m.restore; grow(); }}
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
    }} else if (atBottom && (m.entries || m.cards)) {{
      scroll.scrollTop = scroll.scrollHeight;
    }}
    if (m.focus) box.focus();
    if (missing) post({{ type: "resync" }});
  }}
  function grow() {{ box.style.height = "auto"; box.style.height = Math.min(box.scrollHeight + 2, window.innerHeight * 0.4) + "px"; }}
  box.addEventListener("input", grow);
  box.addEventListener("keydown", (e) => {{
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {{
      e.preventDefault();
      const text = box.value;
      if (!text.trim()) return;
      box.value = ""; grow();
      post({{ type: "send", text }});
    }}
  }});
  document.addEventListener("click", (e) => {{
    const b = e.target.closest("[data-act]");
    if (!b) return;
    const act = b.dataset.act;
    if (act === "fold") {{
      const holder = b.parentElement;
      holder.classList.toggle("open");
      if (holder.classList.contains("open")) opened.add(holder.dataset.id); else opened.delete(holder.dataset.id);
      return;
    }}
    const card = b.closest(".card");
    if (act === "answer" && card) {{
      const field = card.querySelector("textarea");
      const text = field ? field.value : "";
      if (b.dataset.needsText && !text.trim()) {{ if (field) field.focus(); return; }}
      card.querySelectorAll("button").forEach((x) => x.disabled = true);
      post({{ type: "answer", id: card.dataset.id, action: b.dataset.action, text }});
    }} else if (act === "permission" && card) {{
      card.querySelectorAll("button").forEach((x) => x.disabled = true);
      post({{ type: "permission", id: card.dataset.id, choice: b.dataset.choice }});
    }} else if (act === "open" && card) {{
      post({{ type: "open", id: card.dataset.id }});
    }} else if (act === "more" || act === "cancel" || act === "terminal") {{
      post({{ type: act }});
    }}
  }});
  window.kova = {{ update }};
  post({{ type: "ready" }});
}})();
</script></body></html>"##,
        nonce = nonce,
        handler = HANDLER,
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
    fn consecutive_tool_calls_group_across_requests_until_text() {
        let c = conv(vec![
            user("u1", "go"),
            tool("r1", 0, "a"),
            json!({"type": "assistant", "requestId": "r2", "uuid": "r2t", "message": {"content": [{"type": "thinking", "thinking": ""}]}}),
            tool("r2", 1, "b"),
            text("r3", 0, "Done."),
            tool("r4", 0, "c"),
        ]);
        let (entries, more) = entries(&c, EXCHANGES_STEP, false);
        assert!(!more);
        assert_eq!(ids(&entries), ["u:u1", "g:a", "t:r3:0", "g:c"]);
        assert_eq!(entries[1].html.matches(r#"class="tool""#).count(), 2, "a thinking block does not split the run");
        assert!(!entries[1].html.contains("actions"), "five calls or fewer are not folded into a group");
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
        let page = shell(&theme, "<x>", "abc123");
        assert!(page.contains("script-src 'nonce-abc123'"));
        assert_eq!(page.matches("<script").count(), 1);
        assert!(page.contains(r#"<script nonce="abc123">"#));
        assert!(!page.contains("onclick"), "no inline handler: the policy would refuse it anyway");
        assert!(page.contains("<title>&lt;x&gt;</title>"));
    }
}
