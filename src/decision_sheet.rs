//! Decision sheets, native: a list of decisions an agent puts to the user
//! with `kova ask --decisions <file.json>`, answered in Kova's side panel.
//!
//! The format is the `decision-sheet` skill's JSON (title, intro, links,
//! decisions with options, one of them marked `reco`). Kova validates it
//! (anyone who reaches the socket can post one), renders it as a page in the
//! design system's colours, keeps the answers as they are given (a draft on
//! the question, on disk with it), and hands them to the asker as text when
//! the user clicks Done. Everything here is pure and unit-tested; the panel
//! (`window/side_panel.rs`) loads the page and relays its messages.
//!
//! The page is the only one in the panel that runs a script, and the script
//! only reads the form and posts it back: every text of the sheet is escaped
//! into the HTML by Rust, none goes through JavaScript.

use serde::{Deserialize, Serialize};

use crate::window::sidebar::tokens;

/// What the page's script posts its answers under.
pub const HANDLER: &str = "kovaSheet";
/// The choice that means "none of these": the note is the answer.
pub const OTHER: &str = "other";
/// The scheme of a sheet's link on a card: `kova-sheet:<question id>`.
/// Internal to Kova (never on the socket, never loaded as a URL).
pub const LINK_SCHEME: &str = "kova-sheet:";

const MAX_DECISIONS: usize = 60;
const MAX_OPTIONS: usize = 12;
const MAX_LINKS: usize = 20;
const MAX_TEXT: usize = 4000;
const MAX_NOTE: usize = 8000;
/// The largest sheet file `kova ask` reads.
pub const MAX_FILE: u64 = 512 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sheet {
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub intro: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<SheetLink>,
    pub decisions: Vec<SheetDecision>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SheetLink {
    pub label: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SheetDecision {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    pub options: Vec<SheetOption>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SheetOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reco: bool,
}

/// The user's answers so far, in the sheet's order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SheetAnswers {
    #[serde(default)]
    pub decisions: Vec<DecisionAnswer>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionAnswer {
    pub id: String,
    /// An option id, or [`OTHER`]; `None` when only a note was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl SheetAnswers {
    pub fn get(&self, id: &str) -> Option<&DecisionAnswer> {
        self.decisions.iter().find(|a| a.id == id)
    }
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 24 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn text_ok(what: &str, s: &str) -> Result<(), String> {
    if s.chars().count() > MAX_TEXT {
        return Err(format!("{} is longer than {} characters", what, MAX_TEXT));
    }
    Ok(())
}

impl Sheet {
    /// Read and check a sheet file (`kova ask --decisions`).
    pub fn from_json(raw: &str) -> Result<Sheet, String> {
        let sheet: Sheet = serde_json::from_str(raw).map_err(|e| format!("not a decision sheet: {}", e))?;
        sheet.validate()?;
        Ok(sheet)
    }

    /// The limits a sheet must keep to be shown: ids that are plain words,
    /// unique, texts of bounded length, http(s) links, one recommendation at
    /// most per decision.
    pub fn validate(&self) -> Result<(), String> {
        if self.title.trim().is_empty() {
            return Err("the sheet has no title".into());
        }
        text_ok("the title", &self.title)?;
        text_ok("the intro", &self.intro)?;
        if self.links.len() > MAX_LINKS {
            return Err(format!("more than {} links", MAX_LINKS));
        }
        for link in &self.links {
            text_ok("a link label", &link.label)?;
            if !crate::ask::is_panel_url(&link.url) {
                return Err(format!("link '{}' is not an http(s) URL", link.label));
            }
        }
        if self.decisions.is_empty() || self.decisions.len() > MAX_DECISIONS {
            return Err(format!("a sheet holds 1 to {} decisions", MAX_DECISIONS));
        }
        let mut seen = std::collections::HashSet::new();
        for d in &self.decisions {
            if !safe_id(&d.id) {
                return Err(format!("decision id '{}' must be letters, digits, - or _", d.id));
            }
            if !seen.insert(d.id.as_str()) {
                return Err(format!("decision id '{}' appears twice", d.id));
            }
            if d.title.trim().is_empty() {
                return Err(format!("decision {} has no title", d.id));
            }
            text_ok("a decision title", &d.title)?;
            text_ok("a group", d.group.as_deref().unwrap_or(""))?;
            text_ok("a context", d.context.as_deref().unwrap_or(""))?;
            if d.options.is_empty() || d.options.len() > MAX_OPTIONS {
                return Err(format!("decision {} needs 1 to {} options", d.id, MAX_OPTIONS));
            }
            let mut ids = std::collections::HashSet::new();
            for o in &d.options {
                if !safe_id(&o.id) || o.id.eq_ignore_ascii_case(OTHER) {
                    return Err(format!("option id '{}' of {} must be letters, digits, - or _, and not 'other'", o.id, d.id));
                }
                if !ids.insert(o.id.as_str()) {
                    return Err(format!("option id '{}' appears twice in {}", o.id, d.id));
                }
                text_ok("an option label", &o.label)?;
                text_ok("an option detail", o.detail.as_deref().unwrap_or(""))?;
            }
            if d.options.iter().filter(|o| o.reco).count() > 1 {
                return Err(format!("decision {} recommends more than one option", d.id));
            }
        }
        Ok(())
    }

    /// The buttons a one-decision sheet shows on the card: its options'
    /// labels, the recommendation first. `None` when the sheet is answered
    /// in the panel only (several decisions, more than three options, a label
    /// too long for a button).
    pub fn card_choices(&self) -> Option<Vec<String>> {
        let [d] = self.decisions.as_slice() else { return None };
        if d.options.len() > 3 || d.options.iter().any(|o| o.label.chars().count() > 24 || o.label.trim().is_empty()) {
            return None;
        }
        let mut options: Vec<&SheetOption> = d.options.iter().collect();
        options.sort_by_key(|o| !o.reco);
        let labels: Vec<String> = options.iter().map(|o| o.label.trim().to_string()).collect();
        let unique: std::collections::HashSet<&String> = labels.iter().collect();
        (unique.len() == labels.len() && !labels.iter().any(|l| l == crate::ask::OTHER || l == crate::ask::DECISION_DONE)).then_some(labels)
    }

    /// Answers posted by the page, kept to what the sheet asks: known
    /// decisions only, a choice among its options or `other`, notes capped.
    /// The page is Kova's, but what it posts is read as untrusted.
    pub fn answers_from(&self, raw: &serde_json::Value) -> SheetAnswers {
        let cap = |s: &str, n: usize| s.chars().take(n).collect::<String>();
        let posted = raw.get("decisions");
        let mut decisions = Vec::new();
        for d in &self.decisions {
            let Some(a) = posted.and_then(|p| p.get(&d.id)) else { continue };
            let choice = a
                .get("choice")
                .and_then(|c| c.as_str())
                .filter(|c| *c == OTHER || d.options.iter().any(|o| o.id == *c))
                .map(str::to_string);
            let note = cap(a.get("note").and_then(|n| n.as_str()).unwrap_or("").trim(), MAX_NOTE);
            if choice.is_some() || !note.is_empty() {
                decisions.push(DecisionAnswer { id: d.id.clone(), choice, note });
            }
        }
        let note = cap(raw.get("note").and_then(|n| n.as_str()).unwrap_or("").trim(), MAX_NOTE);
        SheetAnswers { decisions, note }
    }

    /// A one-decision sheet answered from the card: `button` is one of
    /// [`card_choices`](Self::card_choices), or `Other` with the text.
    pub fn answers_from_card(&self, button: &str, text: &str) -> Option<SheetAnswers> {
        let [d] = self.decisions.as_slice() else { return None };
        let choice = if button == crate::ask::OTHER {
            OTHER.to_string()
        } else {
            d.options.iter().find(|o| o.label.trim() == button)?.id.clone()
        };
        Some(SheetAnswers {
            decisions: vec![DecisionAnswer { id: d.id.clone(), choice: Some(choice), note: text.trim().to_string() }],
            note: String::new(),
        })
    }

    /// The answers as the asker reads them: one line per decision, in order.
    ///
    /// ```text
    /// D1: A (Ship now)
    /// D2: B (Later). Note: only after the launch
    /// D3: other: ask the team first
    /// D4: open
    /// Note: one more thing
    /// ```
    pub fn summary(&self, answers: &SheetAnswers) -> String {
        let flat = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut lines = Vec::new();
        for d in &self.decisions {
            let a = answers.get(&d.id);
            let note = a.map(|a| flat(&a.note)).unwrap_or_default();
            let line = match a.and_then(|a| a.choice.as_deref()) {
                Some(OTHER) if note.is_empty() => format!("{}: other", d.id),
                Some(OTHER) => format!("{}: other: {}", d.id, note),
                Some(id) => {
                    let label = d.options.iter().find(|o| o.id == id).map(|o| flat(&o.label)).unwrap_or_default();
                    if note.is_empty() { format!("{}: {} ({})", d.id, id, label) } else { format!("{}: {} ({}). Note: {}", d.id, id, label, note) }
                }
                None if note.is_empty() => format!("{}: open", d.id),
                None => format!("{}: open. Note: {}", d.id, note),
            };
            lines.push(line);
        }
        if !answers.note.trim().is_empty() {
            lines.push(format!("Note: {}", flat(&answers.note)));
        }
        lines.join("\n")
    }

    /// How many decisions have an answer (a choice or a note).
    pub fn answered(&self, answers: &SheetAnswers) -> usize {
        self.decisions.iter().filter(|d| answers.get(&d.id).is_some()).count()
    }
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
            // The page's own slots are `@@NAME@@`: a text cannot name one.
            '@' => out.push_str("&#64;"),
            _ => out.push(c),
        }
    }
    out
}

fn hex(c: [f32; 3]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

/// The sheet as a page, with the answers given so far already ticked.
pub fn render(sheet: &Sheet, answers: &SheetAnswers, from: &str, palette: &tokens::Palette) -> String {
    let mut cards = String::new();
    for d in &sheet.decisions {
        if let Some(group) = d.group.as_deref().filter(|g| !g.trim().is_empty()) {
            cards.push_str(&format!("<h2 class=\"group\">{}</h2>\n", esc(group)));
        }
        let a = answers.get(&d.id);
        let chosen = a.and_then(|a| a.choice.as_deref());
        let mut opts = String::new();
        let other = SheetOption { id: OTHER.into(), label: "Other".into(), detail: Some("Say what in the note below".into()), reco: false };
        for o in d.options.iter().chain(std::iter::once(&other)) {
            let num = if o.id == OTHER { String::new() } else { format!("<span class=\"oid\">{}</span>", esc(&o.id)) };
            let reco = if o.reco { "<span class=\"badge\">Recommended</span>" } else { "" };
            let detail = o.detail.as_deref().filter(|t| !t.trim().is_empty()).map(|t| format!("<span class=\"detail\">{}</span>", esc(t))).unwrap_or_default();
            opts.push_str(&format!(
                "<label class=\"opt\"><input type=\"radio\" name=\"d-{did}\" value=\"{oid}\"{checked}{reco_attr}><span class=\"txt\"><span class=\"label\">{num}{label}</span>{reco}{detail}</span></label>\n",
                did = esc(&d.id),
                oid = esc(&o.id),
                checked = if chosen == Some(o.id.as_str()) { " checked" } else { "" },
                reco_attr = if o.reco { " data-reco" } else { "" },
                label = esc(&o.label),
            ));
        }
        let context = d.context.as_deref().filter(|t| !t.trim().is_empty()).map(|t| format!("<p class=\"context\">{}</p>", esc(t))).unwrap_or_default();
        cards.push_str(&format!(
            "<article class=\"card{answered}\" data-id=\"{id}\"><div class=\"head\"><span class=\"num\">{id}</span><h3>{title}</h3></div>{context}<div class=\"opts\">{opts}</div><textarea rows=\"1\" placeholder=\"Add to it, change it or propose something else\" aria-label=\"Note on {id}\">{note}</textarea></article>\n",
            answered = if a.is_some() { " answered" } else { "" },
            id = esc(&d.id),
            title = esc(&d.title),
            note = esc(a.map(|a| a.note.as_str()).unwrap_or("")),
        ));
    }
    let links: String = sheet
        .links
        .iter()
        .map(|l| format!("<a href=\"{}\">{}</a>", esc(&l.url), esc(&l.label)))
        .collect::<Vec<_>>()
        .join("");
    let p = palette;
    let page = PAGE
        .replace("{scheme}", if p == &tokens::LIGHT { "light" } else { "dark" })
        .replace("{bg}", &hex(p.ground))
        .replace("{bg2}", &hex(p.bg2))
        .replace("{tile}", &hex(p.tile))
        .replace("{hover}", &hex(p.tile_hover))
        .replace("{sel}", &hex(p.sel_bg))
        .replace("{t1}", &hex(p.text_primary))
        .replace("{t2}", &hex(p.text_secondary))
        .replace("{t3}", &hex(p.text_tertiary))
        .replace("{b2}", &hex(p.border_strong))
        .replace("{b3}", &hex(p.border_subtle))
        .replace("{accent}", &hex(p.accent))
        .replace("{onfill}", &hex(p.text_on_fill))
        .replace("{ok}", &hex(p.ok_dot))
        .replace("{handler}", HANDLER);
    // The sheet's texts last, so a `{…}` in them is never taken for a slot.
    page.replacen("@@TITLE@@", &esc(&sheet.title), 2)
        .replacen("@@FROM@@", &esc(from), 1)
        .replacen("@@INTRO@@", &esc(&sheet.intro), 1)
        .replacen("@@LINKS@@", &links, 1)
        .replacen("@@NOTE@@", &esc(&answers.note), 1)
        .replacen("@@CARDS@@", &cards, 1)
}

/// What the panel shows once the sheet is answered, or when its question is
/// gone (answered elsewhere, withdrawn).
pub fn render_closed(title: &str, summary: Option<&str>, palette: &tokens::Palette) -> String {
    let p = palette;
    let body = match summary {
        Some(s) => format!(
            "<p class=\"lead\">Answers sent</p><pre>{}</pre>",
            esc(s)
        ),
        None => "<p class=\"lead\">This sheet is no longer waiting for an answer</p>".to_string(),
    };
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>{title}</title><style>
:root {{ color-scheme: {scheme}; }}
body {{ margin: 0; padding: 28px; background: {bg}; color: {t1}; font: 14px/1.55 "Inter", -apple-system, system-ui, sans-serif; }}
h1 {{ font-size: 20px; font-weight: 600; margin: 0 0 14px; }}
.lead {{ color: {ok}; font-weight: 600; margin: 0 0 10px; }}
pre {{ white-space: pre-wrap; font: inherit; color: {t2}; background: {tile}; border: 1px solid {b3}; border-radius: 10px; padding: 12px 14px; margin: 0; }}
</style></head><body><h1>{title}</h1>{body}</body></html>"#,
        title = esc(title),
        scheme = if p == &tokens::LIGHT { "light" } else { "dark" },
        bg = hex(p.ground),
        t1 = hex(p.text_primary),
        t2 = hex(p.text_secondary),
        tile = hex(p.tile),
        b3 = hex(p.border_subtle),
        ok = hex(p.ok_dot),
        body = body,
    )
}

const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>@@TITLE@@</title>
<style>
:root { color-scheme: {scheme}; }
* { box-sizing: border-box; }
html, body { background: {bg}; }
body { margin: 0; color: {t1}; font: 14px/1.5 "Inter", -apple-system, system-ui, sans-serif; }
.wrap { max-width: 760px; margin: 0 auto; padding: 0 20px; }
header { padding: 22px 0 14px; }
h1 { font-size: 20px; line-height: 1.25; font-weight: 600; margin: 0 0 4px; }
.from { color: {t3}; font-size: 12px; margin: 0 0 8px; }
.intro { color: {t2}; margin: 0; }
.links { display: flex; flex-wrap: wrap; gap: 4px 14px; margin-top: 8px; }
a { color: {accent}; text-decoration: none; } a:hover { text-decoration: underline; }
.bar { position: sticky; top: 0; z-index: 5; background: {bg}; border-bottom: 1px solid {b3}; }
.bar .wrap { display: flex; align-items: center; gap: 12px; padding-top: 8px; padding-bottom: 8px; }
.count { font-weight: 600; font-variant-numeric: tabular-nums; font-size: 13px; white-space: nowrap; }
.track { flex: 1; height: 4px; background: {b3}; border-radius: 2px; overflow: hidden; }
.fill { height: 100%; width: 0; background: {accent}; transition: width .2s; }
button { font: 500 13px/1 "Inter", -apple-system, system-ui, sans-serif; cursor: pointer; height: 30px; padding: 0 12px;
  border-radius: 8px; border: 1px solid {b2}; background: {hover}; color: {t1}; white-space: nowrap; }
button:hover { border-color: {accent}; }
button.primary { background: {accent}; border-color: {accent}; color: {onfill}; }
button.on { border-color: {accent}; color: {accent}; }
:focus-visible { outline: 2px solid {accent}; outline-offset: 2px; }
main { display: grid; gap: 12px; padding: 16px 0 8px; }
h2.group { font-size: 11px; letter-spacing: .06em; text-transform: uppercase; color: {t3}; margin: 10px 0 -2px; font-weight: 600; }
.card { background: {tile}; border: 1px solid {b3}; border-radius: 12px; padding: 14px; display: grid; gap: 10px; }
.card.answered { border-color: {b2}; }
.only-open .card.answered { display: none; }
.head { display: flex; gap: 8px; align-items: baseline; }
.num { color: {t3}; font-weight: 600; font-size: 12px; font-variant-numeric: tabular-nums; flex: none; }
.head h3 { margin: 0; font-size: 15px; line-height: 1.35; font-weight: 600; }
.context { color: {t2}; margin: 0; font-size: 13px; }
.opts { display: grid; gap: 6px; }
.opt { display: grid; grid-template-columns: 16px 1fr; gap: 9px; padding: 8px 10px; border: 1px solid {b3}; border-radius: 8px; cursor: pointer; background: {bg}; }
.opt:hover { border-color: {b2}; }
.opt:has(input:checked) { background: {sel}; border-color: {accent}; }
.opt input { margin: 2px 0 0; accent-color: {accent}; }
.label { font-weight: 500; }
.oid { color: {t3}; font-weight: 600; margin-right: 6px; }
.badge { display: inline-block; font-size: 11px; font-weight: 600; color: {ok}; border: 1px solid {ok}; border-radius: 5px; padding: 0 5px; margin-left: 6px; vertical-align: 1px; }
.detail { color: {t2}; font-size: 13px; display: block; }
textarea { width: 100%; font: inherit; font-size: 13px; color: {t1}; background: {bg}; border: 1px solid {b3}; border-radius: 8px; padding: 7px 10px; min-height: 34px; resize: vertical; }
textarea:focus { border-color: {accent}; outline: none; }
.end { display: grid; gap: 10px; padding: 8px 0 40px; }
.end h2 { font-size: 15px; font-weight: 600; margin: 6px 0 0; }
.row { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
.hint { color: {t3}; font-size: 12px; }
</style></head><body>
<div class="wrap"><header><h1>@@TITLE@@</h1><p class="from">From @@FROM@@</p><p class="intro">@@INTRO@@</p><div class="links">@@LINKS@@</div></header></div>
<div class="bar"><div class="wrap"><span class="count" id="count"></span><div class="track"><div class="fill" id="fill"></div></div><button id="filter" type="button">Open only</button></div></div>
<div class="wrap">
<main id="cards">
@@CARDS@@</main>
<section class="end">
<div class="row" id="recoBox"></div>
<h2><label for="general">Anything else?</label></h2>
<textarea id="general" rows="3" placeholder="A default to challenge, a missing decision, an instruction for what comes next">@@NOTE@@</textarea>
<div class="row"><button class="primary" id="done" type="button">Done</button><span class="hint" id="hint">Saved as you go</span></div>
</section></div>
<script>
const $ = id => document.getElementById(id);
const cards = [...document.querySelectorAll('.card')];
function collect() {
  const decisions = {};
  cards.forEach(c => {
    const r = c.querySelector('input:checked');
    const note = c.querySelector('textarea').value;
    if (r || note.trim()) decisions[c.dataset.id] = { choice: r ? r.value : null, note };
  });
  return { decisions, note: $('general').value };
}
function post(type) {
  try { window.webkit.messageHandlers.{handler}.postMessage(JSON.stringify(Object.assign({ type }, collect()))); } catch (e) {}
}
function progress() {
  const n = cards.filter(c => c.querySelector('input:checked') || c.querySelector('textarea').value.trim()).length;
  cards.forEach(c => c.classList.toggle('answered', !!(c.querySelector('input:checked') || c.querySelector('textarea').value.trim())));
  $('count').textContent = n + ' of ' + cards.length;
  $('fill').style.width = (100 * n / cards.length) + '%';
  recoBox();
}
let timer = null;
function later() { clearTimeout(timer); timer = setTimeout(() => post('draft'), 500); }
function recoBox() {
  const box = $('recoBox');
  box.textContent = '';
  const open = cards.filter(c => !c.classList.contains('answered') && c.querySelector('input[data-reco]'));
  if (!open.length) return;
  const b = document.createElement('button');
  b.type = 'button';
  b.textContent = 'Take the recommendation on the ' + open.length + ' left';
  b.onclick = () => {
    box.textContent = '';
    const q = document.createElement('span'); q.className = 'hint';
    q.textContent = open.map(c => c.dataset.id).join(', ') + ': confirm?';
    const yes = document.createElement('button'); yes.type = 'button'; yes.className = 'primary'; yes.textContent = 'Confirm';
    yes.onclick = () => { open.forEach(c => { c.querySelector('input[data-reco]').checked = true; }); progress(); post('draft'); };
    const no = document.createElement('button'); no.type = 'button'; no.textContent = 'Cancel'; no.onclick = recoBox;
    box.append(q, yes, no);
  };
  box.append(b);
}
document.addEventListener('change', e => { if (e.target.type === 'radio') { progress(); post('draft'); if (e.target.value === 'other') e.target.closest('.card').querySelector('textarea').focus(); } });
document.addEventListener('input', e => { if (e.target.tagName === 'TEXTAREA') { progress(); later(); } });
$('filter').onclick = () => { const on = document.body.classList.toggle('only-open'); $('filter').classList.toggle('on', on); };
$('done').onclick = () => { clearTimeout(timer); post('done'); $('hint').textContent = 'Sending'; };
progress();
</script></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet() -> Sheet {
        Sheet::from_json(
            r#"{"title":"Voice typing","intro":"Pick <b>one</b>","links":[{"label":"Ticket","url":"https://linear.app/x"}],
            "decisions":[
              {"id":"D1","title":"Ship when?","group":"Timing","options":[{"id":"A","label":"Now","reco":true},{"id":"B","label":"Later","detail":"after launch"}]},
              {"id":"D2","title":"Who owns it?","context":"Nobody yet","options":[{"id":"A","label":"Alice"},{"id":"B","label":"Bob"}]}
            ]}"#,
        )
        .unwrap()
    }

    #[test]
    fn the_skill_format_reads_as_is() {
        let s = sheet();
        assert_eq!(s.decisions.len(), 2);
        assert_eq!(s.decisions[0].group.as_deref(), Some("Timing"));
        assert!(s.decisions[0].options[0].reco);
    }

    #[test]
    fn a_sheet_out_of_bounds_is_refused() {
        let bad = |json: &str| Sheet::from_json(json).is_err();
        assert!(bad(r#"{"title":"","decisions":[{"id":"D1","title":"x","options":[{"id":"A","label":"a"}]}]}"#));
        assert!(bad(r#"{"title":"t","decisions":[]}"#));
        assert!(bad(r#"{"title":"t","decisions":[{"id":"D 1","title":"x","options":[{"id":"A","label":"a"}]}]}"#), "id with a space");
        assert!(bad(r#"{"title":"t","decisions":[{"id":"D1","title":"x","options":[{"id":"A","label":"a"}]},{"id":"D1","title":"y","options":[{"id":"A","label":"a"}]}]}"#), "twice");
        assert!(bad(r#"{"title":"t","decisions":[{"id":"D1","title":"x","options":[{"id":"other","label":"a"}]}]}"#), "reserved");
        assert!(bad(r#"{"title":"t","decisions":[{"id":"D1","title":"x","options":[{"id":"A","label":"a","reco":true},{"id":"B","label":"b","reco":true}]}]}"#), "two recos");
        assert!(bad(r#"{"title":"t","links":[{"label":"x","url":"javascript:alert(1)"}],"decisions":[{"id":"D1","title":"x","options":[{"id":"A","label":"a"}]}]}"#));
    }

    #[test]
    fn posted_answers_keep_only_what_the_sheet_asks() {
        let s = sheet();
        let raw = serde_json::json!({
            "decisions": {
                "D1": {"choice": "B", "note": "  only after  "},
                "D2": {"choice": "Z", "note": ""},
                "D9": {"choice": "A"}
            },
            "note": "more"
        });
        let a = s.answers_from(&raw);
        assert_eq!(a.decisions, vec![DecisionAnswer { id: "D1".into(), choice: Some("B".into()), note: "only after".into() }]);
        assert_eq!(a.note, "more");
    }

    #[test]
    fn the_summary_reads_one_line_per_decision() {
        let s = sheet();
        let a = SheetAnswers {
            decisions: vec![
                DecisionAnswer { id: "D1".into(), choice: Some("B".into()), note: "after\nlaunch".into() },
            ],
            note: "thanks".into(),
        };
        assert_eq!(s.summary(&a), "D1: B (Later). Note: after launch\nD2: open\nNote: thanks");
        let other = SheetAnswers { decisions: vec![DecisionAnswer { id: "D2".into(), choice: Some(OTHER.into()), note: "Carol".into() }], note: String::new() };
        assert_eq!(s.summary(&other), "D1: open\nD2: other: Carol");
        assert_eq!(s.answered(&other), 1);
    }

    #[test]
    fn one_short_decision_is_answered_on_the_card() {
        let mut s = sheet();
        assert_eq!(s.card_choices(), None, "two decisions: the panel");
        s.decisions.truncate(1);
        s.decisions[0].options.swap(0, 1);
        assert_eq!(s.card_choices(), Some(vec!["Now".to_string(), "Later".to_string()]), "the recommendation first");
        let a = s.answers_from_card("Later", "").unwrap();
        assert_eq!(s.summary(&a), "D1: B (Later)");
        let a = s.answers_from_card(crate::ask::OTHER, "next month").unwrap();
        assert_eq!(s.summary(&a), "D1: other: next month");
        assert_eq!(s.answers_from_card("Nope", ""), None);
    }

    #[test]
    fn the_page_escapes_every_text_of_the_sheet() {
        let mut s = sheet();
        s.decisions[1].title = "</script><script>alert(1)</script>{bg}".into();
        let a = SheetAnswers { decisions: vec![DecisionAnswer { id: "D1".into(), choice: Some("A".into()), note: "<i>".into() }], note: String::new() };
        let html = render(&s, &a, "agent<x>", &tokens::DARK);
        assert!(!html.contains("<script>alert"));
        assert!(html.contains("&lt;/script&gt;&lt;script&gt;alert(1)&lt;/script&gt;{bg}"), "a slot name in a text stays text");
        assert!(html.contains("Pick &lt;b&gt;one&lt;/b&gt;"));
        assert!(html.contains("agent&lt;x&gt;"));
        assert!(html.contains("value=\"A\" checked data-reco"), "the draft is ticked");
        assert!(html.contains("&lt;i&gt;</textarea>"));
        assert!(html.contains("messageHandlers.kovaSheet"));
        assert_eq!(html.matches("<script>").count(), 1, "one script: Kova's");
    }
}
