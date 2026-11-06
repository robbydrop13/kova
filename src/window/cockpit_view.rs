//! The Cockpit page over the panes (`docs/cockpit.md`, the pure half in
//! `crate::cockpit`).
//!
//! One web view per window, created on first use: the sidebar's Cockpit entry
//! opens it over the whole panes area of the active tab, a second click or
//! Escape closes it, and it steps aside while a Metal overlay (the switcher,
//! the palette, help) needs the window, like the chat. The tick gathers the
//! actions (the need items it already built, the attention queue as last
//! read), renders the page in Rust and pushes only the parts that changed;
//! the page keeps a draft being rewritten through a push.
//!
//! The `Chat` button opens a chief of staff's chat on the right third of the
//! page: the chat mode's own web view (`chat_view.rs`), pointed at the chief's
//! pane through `cockpit_chat_target`, with no focus or tab switch. A message
//! is taken only from this web view's main frame on `about:blank`.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSView, NSWindowOrderingMode, NSWorkspace};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSNumber, NSObject, NSObjectProtocol, NSString};
use objc2_web_kit::{
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationType, WKScriptMessage,
    WKScriptMessageHandler, WKUserContentController, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
};
use serde_json::Value;

use crate::attention::{Scope, Verb};
use crate::cockpit::{self, Button, ChatColumn, Detail, Filter, Item, Kind, RoutineAct, State, Update, View};
use crate::routines::Routine;
use crate::surface::PaneId;

use super::sidebar::tokens;
use super::KovaView;

/// How often the actions are gathered again while the page is open.
const POLL_MS: u128 = 250;
/// A page message longer than this is not one the page sends (a draft is
/// capped at `attention::MAX_TEXT`, well under it).
const MESSAGE_MAX: usize = 64 * 1024;
/// The chat column's top bar, in points: the chat's web view sits under it.
const CHAT_TOP_H: f64 = 56.0;
/// How long a typed permission answer keeps its card waiting.
const PENDING_MS: u64 = 5_000;
/// The pane lines an opened error card shows.
const ERROR_LINES: usize = 15;
/// The most of a routine's prompt an opened card shows.
const PROMPT_MAX: usize = 16 * 1024;
/// How long a removed, done or snoozed card stays hidden waiting for the
/// queue to drop it.
const HIDDEN_MS: u64 = 60_000;

/// A frame in the container's coordinates (points, y up).
type Frame = (f64, f64, f64, f64);

/// The window's Cockpit: its web view and what the page holds.
#[derive(Default)]
pub struct CockpitSlot {
    web: OnceCell<Retained<WKWebView>>,
    delegate: OnceCell<Retained<CockpitWebDelegate>>,
    open: Cell<bool>,
    chat: Cell<bool>,
    chief: Cell<Scope>,
    filter: Cell<Filter>,
    frame: Cell<Option<Frame>>,
    /// The chat's place this tick, read by `chat_view`.
    chat_target: Cell<Option<(PaneId, Frame)>>,
    /// Give the page the keyboard on the next tick.
    focus: Cell<bool>,
    /// The cards opened with `Ouvrir`, by key: kept through every refresh,
    /// dropped once the card is gone.
    expanded: RefCell<HashSet<String>>,
    /// How many cards the list shows (`Voir plus` adds a page); 0 is one page.
    limit: Cell<usize>,
    /// Entries removed, done or snoozed from here, by key, with when (ms):
    /// hidden until the queue drops them, back if the command failed.
    hidden: RefCell<HashMap<String, u64>>,
    /// The catch-up band: folded with its chevron, every row shown after
    /// `Voir plus`, the project rows opened in place.
    band_folded: Cell<bool>,
    band_all: Cell<bool>,
    band_open: RefCell<HashSet<String>>,
    /// A read of the band landed during this visit: leaving marks it read.
    band_read: Cell<bool>,
    /// The card a waiting item of the band points at, for the next push.
    reveal: RefCell<Option<String>>,
    page: RefCell<Page>,
}

/// What the loaded page holds.
#[derive(Default)]
struct Page {
    /// The nonce of the page loaded; a message carrying another one comes
    /// from an earlier load.
    load: String,
    ready: bool,
    shown: Option<(String, String, String)>,
    last_poll: Option<std::time::Instant>,
    /// The items of the last render: what a click names.
    items: Vec<Item>,
    /// The theme the page was loaded in (`theme::generation`).
    theme: u64,
    /// A permission card's prompt as rendered, by key: a click answers it
    /// only if the screen still shows that prompt.
    prompts: HashMap<String, String>,
    /// Permission and question answers given, by key, with when (ms): the
    /// card waits.
    pending: HashMap<String, u64>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Whether a pane runs the chief's session: its peer name or its agent
/// session's name, like `attn`'s `type_into_session`.
fn holds_session(pane: &crate::pane::Pane, name: &str) -> bool {
    pane.claude_peer_name().as_deref() == Some(name) || pane.agent_session_name().as_deref() == Some(name)
}

impl KovaView {
    /// The sidebar's Cockpit entry: open the page, or close it.
    pub(super) fn toggle_cockpit(&self) {
        let slot = &self.ivars().cockpit;
        if slot.open.get() {
            self.close_cockpit();
        } else {
            slot.open.set(true);
            slot.focus.set(true);
            slot.limit.set(cockpit::PAGE_SIZE);
            crate::attention::refresh();
            slot.band_all.set(false);
            slot.band_open.borrow_mut().clear();
            slot.band_read.set(false);
            crate::catchup::refresh();
        }
        self.mark_dirty();
    }

    pub(super) fn close_cockpit(&self) {
        let slot = &self.ivars().cockpit;
        if !slot.open.get() {
            return;
        }
        self.cockpit_leave(false);
        slot.open.set(false);
        slot.chat_target.set(None);
        self.hide_cockpit();
    }

    /// The user leaves the page (it closes, or its window does): the
    /// catch-up band is marked read, if one was shown. `blocking` waits for
    /// `attn seen` (the app is quitting).
    pub fn cockpit_leave(&self, blocking: bool) {
        let slot = &self.ivars().cockpit;
        if crate::catchup::seen_on_leave(slot.open.get(), slot.band_read.replace(false)) {
            crate::catchup::mark_seen(blocking);
        }
    }

    pub(super) fn cockpit_open(&self) -> bool {
        self.ivars().cockpit.open.get()
    }

    /// The chief's pane and where its chat goes, while the page shows the
    /// chat column: `chat_view` puts the chat there instead of over a pane.
    pub(super) fn cockpit_chat_target(&self) -> Option<(PaneId, Frame)> {
        self.ivars().cockpit.chat_target.get()
    }

    /// The chat's `terminal` button, from the Cockpit: the column closes.
    pub(super) fn cockpit_close_chat(&self) {
        self.ivars().cockpit.chat.set(false);
        self.ivars().cockpit.focus.set(true);
        self.ivars().cockpit.chat_target.set(None);
    }

    /// The page's web view, once made: the chat goes above it.
    pub(super) fn cockpit_web(&self) -> Option<Retained<WKWebView>> {
        self.ivars().cockpit.web.get().cloned()
    }

    /// The chief this window's chat talks to, for the session file.
    pub fn cockpit_chief(&self) -> Option<String> {
        (self.ivars().cockpit.chief.get() == Scope::Perso).then(|| "perso".to_string())
    }

    pub fn set_cockpit_chief(&self, chief: Option<&str>) {
        self.ivars().cockpit.chief.set(if chief == Some("perso") { Scope::Perso } else { Scope::Pro });
    }

    /// Whether one action is urgent, for the entry's red dot.
    pub(super) fn cockpit_urgent(&self) -> bool {
        cockpit::any_urgent(&self.cockpit_items(&|_| false))
    }

    /// Every action, sorted: the tick's need items but the attention ones,
    /// which the queue's own entries stand for (`now` and `point`, with their
    /// priority, scope and draft). `exists` decides whether an entry's file
    /// can be opened.
    fn cockpit_items(&self, exists: &dyn Fn(&Path) -> bool) -> Vec<Item> {
        let mtm = MainThreadMarker::from(self);
        let mut sheets = std::collections::HashSet::new();
        let mut waiting = Vec::new();
        crate::app::app_delegate(mtm).ivars().for_each_waiting_ask(|q| {
            if q.panel_link().is_some() {
                sheets.insert(format!("ask:{}", q.id));
            }
            waiting.push(q.id.clone());
        });
        let mut actions = Vec::new();
        let mut items: Vec<Item> = {
            let st = self.ivars().sidebar.borrow();
            st.needs.iter().filter_map(|need| cockpit::from_need(need, sheets.contains(&need.key))).collect()
        };
        if let Some(entries) = crate::attention::snapshot() {
            let outcomes = crate::attention::outcomes();
            let instructions = crate::attention::instructions();
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
            for entry in entries.iter() {
                let openable = entry
                    .action
                    .as_deref()
                    .is_some_and(|a| super::sidebar_attention::open_target(a, home.as_deref(), exists).is_some());
                items.extend(cockpit::from_attention(entry, outcomes.get(&entry.id), openable).map(|mut item| {
                    item.instruction = instructions.get(&entry.id).cloned();
                    item
                }));
                if let Some(a) = &entry.action {
                    actions.push((format!("attention:{}", entry.id), a.clone()));
                }
            }
        }
        cockpit::link_asks(&mut items, &actions, &waiting);
        cockpit::sort(&mut items);
        items
    }

    /// The panes area of the active tab, on screen, in the container's
    /// coordinates.
    fn cockpit_frame(&self) -> Option<Frame> {
        let tabs = self.ivars().tabs.borrow();
        let tab = tabs.get(self.ivars().active_tab.get())?;
        let vp = self.panes_viewport_for_tab(tab);
        let scale = self.backing_scale().max(1.0) as f64;
        let view = self.frame();
        let top = (vp.y as f64 / scale).max(0.0);
        let h = (vp.height as f64 / scale).min(view.size.height - top).max(0.0);
        let w = view.size.width;
        if w < 80.0 || h < 80.0 {
            return None;
        }
        Some((view.origin.x, view.origin.y + view.size.height - top - h, w, h))
    }

    fn hide_cockpit(&self) {
        let Some(web) = self.ivars().cockpit.web.get() else { return };
        if !web.isHidden() {
            if let Some(window) = self.window()
                && window.firstResponder().is_some_and(|r| r.downcast_ref::<NSView>().is_some_and(|v| v.isDescendantOf(web)))
            {
                window.makeFirstResponder(Some(self));
            }
            web.setHidden(true);
        }
    }

    /// Once per frame: place the page (or hide it), serve its messages, push
    /// what changed, and say where the chief's chat goes.
    pub(super) fn tick_cockpit(&self) {
        let slot = &self.ivars().cockpit;
        let frame = if slot.open.get() && !self.metal_overlay_open() { self.cockpit_frame() } else { None };
        let Some(frame) = frame else {
            slot.chat_target.set(None);
            self.hide_cockpit();
            return;
        };
        let Some(window) = self.window() else { return };
        let mtm = MainThreadMarker::from(self);
        let web = self.cockpit_web_view(mtm);
        let crashed = slot.delegate.get().is_some_and(|d| d.ivars().crashed.take());
        let theme = crate::theme::generation();
        if crashed || slot.page.borrow().load.is_empty() || slot.page.borrow().theme != theme {
            let nonce = crate::ask::new_question_id().replace('-', "");
            *slot.page.borrow_mut() = Page { load: nonce.clone(), theme, ..Default::default() };
            let html = cockpit::shell(tokens::get() == &tokens::LIGHT, &nonce);
            unsafe { web.loadHTMLString_baseURL(&NSString::from_str(&html), None) };
        }
        if slot.frame.get() != Some(frame) {
            let (x, y, w, h) = frame;
            web.setFrame(CGRect { origin: CGPoint { x, y }, size: CGSize { width: w, height: h } });
            slot.frame.set(Some(frame));
        }
        if web.isHidden() {
            web.setHidden(false);
        }
        // The panes are covered: the keyboard never stays on the terminal
        // under the page (the chief's chat keeps it when it has it).
        let on_terminal = window
            .firstResponder()
            .is_some_and(|r| std::ptr::eq(Retained::as_ptr(&r).cast::<()>(), (self as *const Self).cast::<()>()));
        if slot.focus.take() || on_terminal {
            window.makeFirstResponder(Some(&web));
        }

        let messages = slot.delegate.get().map(|d| d.ivars().take()).unwrap_or_default();
        let mut force = false;
        for raw in messages {
            force |= self.cockpit_message(&raw);
        }
        if !slot.open.get() {
            return;
        }

        // The chief's chat on the right third, under the column's top bar.
        let chief = slot.chief.get();
        let chief_pane = if slot.chat.get() { self.chief_pane(cockpit::chief_session(chief)) } else { None };
        slot.chat_target.set(chief_pane.map(|id| {
            let (x, y, w, h) = frame;
            let col = (w / 3.0).round();
            (id, (x + w - col + 1.0, y, col - 1.0, (h - CHAT_TOP_H).max(0.0)))
        }));

        let due = slot.page.borrow().last_poll.is_none_or(|t| t.elapsed().as_millis() >= POLL_MS);
        if !(due || force) {
            return;
        }
        let mut items = self.cockpit_items(&|p: &Path| p.is_file());
        {
            let now = now_ms();
            let mut hidden = slot.hidden.borrow_mut();
            hidden.retain(|k, at| now.saturating_sub(*at) < HIDDEN_MS && items.iter().any(|i| &i.key == k));
            // A command that failed brings its card back, with its words.
            hidden.retain(|k, _| !items.iter().any(|i| &i.key == k && matches!(i.state, State::Failed(_))));
            items.retain(|i| !hidden.contains_key(&i.key));
        }
        self.cockpit_fill(&mut items);
        let mut page = slot.page.borrow_mut();
        page.last_poll = Some(std::time::Instant::now());
        let now = now_ms();
        page.pending.retain(|_, at| now.saturating_sub(*at) < PENDING_MS);
        let mut prompts = HashMap::new();
        for item in &mut items {
            if page.pending.contains_key(&item.key) {
                item.state = State::Running;
            }
            if item.kind != Kind::Permission {
                continue;
            }
            let prompt = pane_of(&item.key).and_then(|id| {
                super::sidebar_ui::with_pane(id, |p| p.permission_prompt().map(|pr| crate::chat::cards::permission_id(&pr)))
                    .flatten()
            });
            if let Some(prompt) = prompt {
                prompts.insert(item.key.clone(), prompt);
            }
        }
        page.prompts = prompts;
        let view = View {
            items: &items,
            filter: slot.filter.get(),
            chat: slot.chat.get().then_some(ChatColumn { chief, found: chief_pane.is_some() }),
            can_act: crate::attention::can_act(),
            can_close: crate::attention::can_close(),
            can_snooze: crate::attention::can_snooze(),
            limit: slot.limit.get().max(cockpit::PAGE_SIZE),
            now: now / 1000,
            band: self.cockpit_band(&items),
        };
        let parts = cockpit::parts(&view);
        page.items = items;
        if !page.ready {
            return;
        }
        let shown = page.shown.clone().unwrap_or_default();
        let fresh = page.shown.is_none();
        let update = Update {
            head: (fresh || shown.0 != parts.0).then(|| parts.0.clone()),
            list: (fresh || shown.1 != parts.1).then(|| parts.1.clone()),
            chat: (fresh || shown.2 != parts.2).then(|| parts.2.clone()),
            focus: false,
            reveal: slot.reveal.borrow_mut().take(),
        };
        page.shown = Some(parts);
        drop(page);
        if !update.is_empty() {
            unsafe { web.evaluateJavaScript_completionHandler(&NSString::from_str(&cockpit::update_script(&update)), None) };
        }
    }

    /// The catch-up band as the page shows it: empty when nothing moved or
    /// the read has not landed.
    fn cockpit_band(&self, items: &[Item]) -> String {
        let slot = &self.ivars().cockpit;
        let Some(data) = crate::catchup::snapshot() else { return String::new() };
        slot.band_read.set(true);
        let opened = slot.band_open.borrow();
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let exists = |p: &Path| p.is_file();
        let opens = |l: &str| super::sidebar_attention::open_target(l, home.as_deref(), &exists).is_some();
        let is_card = |k: &str| items.iter().any(|i| i.key == k);
        crate::catchup::band_html(&crate::catchup::Band {
            data: &data,
            folded: slot.band_folded.get(),
            opened: &opened,
            all: slot.band_all.get(),
            is_card: &is_card,
            opens: &opens,
            clock: &crate::catchup::local_clock,
        })
    }

    /// A click in the band: `Tout lu`, the chevron, `Voir plus`, a row, a
    /// link (only one the band shows, in the side panel like the cards').
    fn cockpit_band_click(&self, what: &str, project: &str, link: &str) {
        let slot = &self.ivars().cockpit;
        match what {
            "seen" => {
                crate::catchup::mark_seen(false);
                slot.band_read.set(false);
                slot.band_open.borrow_mut().clear();
            }
            "fold" => slot.band_folded.set(!slot.band_folded.get()),
            "all" => slot.band_all.set(true),
            "row" => {
                let mut open = slot.band_open.borrow_mut();
                if !open.remove(project) {
                    open.insert(project.to_string());
                }
            }
            "link" => {
                if crate::catchup::snapshot().is_some_and(|d| crate::catchup::shows_link(&d, project, link)) {
                    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
                    let exists = |p: &Path| p.is_file();
                    if let Some(target) = super::sidebar_attention::open_target(link, home.as_deref(), &exists) {
                        self.open_in_side_panel(target);
                    }
                }
            }
            _ => {}
        }
        self.mark_dirty();
    }

    /// A waiting item of the band that is a card: the list shows it (the
    /// filter and `Voir plus` make room) and the page scrolls to it.
    fn cockpit_reveal(&self, key: &str) {
        let slot = &self.ivars().cockpit;
        let page = slot.page.borrow();
        let Some(item) = page.items.iter().find(|i| i.key == key) else { return };
        if let Filter::Only(scope) = slot.filter.get()
            && scope != item.scope
        {
            slot.filter.set(Filter::All);
        }
        let filter = slot.filter.get();
        let at = page
            .items
            .iter()
            .filter(|i| filter == Filter::All || matches!(filter, Filter::Only(s) if s == i.scope))
            .position(|i| i.key == key)
            .unwrap_or(0);
        let mut limit = slot.limit.get().max(cockpit::PAGE_SIZE);
        while limit <= at {
            limit += cockpit::PAGE_SIZE;
        }
        slot.limit.set(limit);
        *slot.reveal.borrow_mut() = Some(key.to_string());
    }

    /// The pane of this window that runs `name`, if any.
    fn chief_pane(&self, name: &str) -> Option<PaneId> {
        let tabs = self.ivars().tabs.borrow();
        let mut found = None;
        for tab in tabs.iter() {
            tab.for_each_pane(&mut |p| {
                if found.is_none() && holds_session(p, name) {
                    found = Some(p.id);
                }
            });
        }
        found
    }

    /// One message from the page. True when the page needs a push now.
    fn cockpit_message(&self, raw: &str) -> bool {
        let slot = &self.ivars().cockpit;
        let Ok(msg) = serde_json::from_str::<Value>(raw) else { return false };
        let s = |k: &str| msg.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        if s("load") != slot.page.borrow().load {
            return false;
        }
        match s("type").as_str() {
            "ready" => {
                let mut page = slot.page.borrow_mut();
                page.ready = true;
                page.shown = None;
                true
            }
            "close" => {
                self.close_cockpit();
                self.mark_dirty();
                false
            }
            "filter" => {
                if let Some(filter) = Filter::parse(&s("scope")) {
                    slot.filter.set(filter);
                    slot.limit.set(cockpit::PAGE_SIZE);
                }
                true
            }
            "band" => {
                self.cockpit_band_click(&s("what"), &s("proj"), &s("link"));
                true
            }
            "reveal" => {
                self.cockpit_reveal(&s("key"));
                true
            }
            "more" => {
                slot.limit.set(slot.limit.get().max(cockpit::PAGE_SIZE) + cockpit::PAGE_SIZE);
                true
            }
            "chief" => {
                slot.chief.set(if s("scope") == "perso" { Scope::Perso } else { Scope::Pro });
                true
            }
            "chat" => {
                slot.chat.set(s("what") == "toggle" && !slot.chat.get());
                slot.focus.set(!slot.chat.get());
                true
            }
            "act" => {
                let text = msg.get("text").and_then(Value::as_str).map(str::to_string);
                let key = s("key");
                match s("act").as_str() {
                    // The buttons inside an opened card.
                    "routine" => self.cockpit_routine(&key, &s("arg")),
                    "answer" => self.cockpit_answer(&key, &s("arg"), text.as_deref().unwrap_or_default()),
                    "link" => self.cockpit_link(&key, &s("arg")),
                    "instruct" => self.cockpit_instruct(&key, text.as_deref().unwrap_or_default()),
                    "pane" => {
                        self.close_cockpit();
                        self.sidebar_open_need(&key);
                        self.mark_dirty();
                    }
                    act => {
                        if let Some(button) = Button::parse(act) {
                            self.cockpit_act(&key, button, text.as_deref());
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// Open a waiting ask's sheet (or page) in the side panel, images and all.
    fn cockpit_open_ask(&self, id: &str) {
        let mut link = None;
        let mtm = MainThreadMarker::from(self);
        crate::app::app_delegate(mtm).ivars().for_each_waiting_ask(|q| {
            if q.id == id {
                link = q.panel_link();
            }
        });
        if let Some(link) = link {
            crate::ask_card::open_in_panel(id.to_string(), link);
        }
    }

    /// A card's button, on the item it was rendered from.
    fn cockpit_act(&self, key: &str, button: Button, text: Option<&str>) {
        let slot = &self.ivars().cockpit;
        let Some(item) = slot.page.borrow().items.iter().find(|i| i.key == key).cloned() else { return };
        let attention_id = key.strip_prefix("attention:");
        match (item.kind, button) {
            (Kind::Attention, Button::Open | Button::OpenSheet) if item.ask.is_some() => {
                self.cockpit_open_ask(item.ask.as_deref().unwrap_or_default());
            }
            (Kind::Attention, Button::Open) => {
                let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
                let action = crate::attention::snapshot()
                    .and_then(|entries| entries.iter().find(|e| Some(e.id.as_str()) == attention_id).and_then(|e| e.action.clone()));
                let exists = |p: &Path| p.is_file();
                if let Some(target) =
                    action.and_then(|a| super::sidebar_attention::open_target(&a, home.as_deref(), &exists))
                {
                    self.open_in_side_panel(target);
                }
            }
            (Kind::Attention, Button::Done | Button::Dismiss | Button::Snooze | Button::Validate | Button::Correct) => {
                let Some(id) = attention_id else { return };
                let (verb, text) = match button {
                    Button::Done => (Verb::Resolve, None),
                    Button::Dismiss => (Verb::Dismiss, None),
                    Button::Snooze => (Verb::Snooze, text),
                    Button::Validate => (Verb::Validate, None),
                    _ => (Verb::Correct, text),
                };
                match crate::attention::perform_with(id, verb, text) {
                    // Gone from the page at once; the next read drops it.
                    Ok(()) if matches!(verb, Verb::Resolve | Verb::Dismiss | Verb::Snooze) => {
                        slot.hidden.borrow_mut().insert(key.to_string(), now_ms());
                    }
                    Ok(()) => {}
                    Err(why) => log::warn!("cockpit: {why}"),
                }
            }
            (Kind::Sheet, Button::Open | Button::OpenSheet) => {
                self.cockpit_open_ask(key.strip_prefix("ask:").unwrap_or_default());
            }
            // The card opens in place, or folds back.
            (_, Button::Expand) => {
                let opened = {
                    let mut expanded = slot.expanded.borrow_mut();
                    !expanded.remove(key) && expanded.insert(key.to_string())
                };
                // An entry that says nothing: `attn explain` writes its
                // detail meanwhile, once at a time.
                if opened
                    && let Some(id) = attention_id
                    && crate::attention::snapshot()
                        .is_some_and(|entries| entries.iter().any(|e| e.id == id && cockpit::says_nothing(e)))
                {
                    crate::attention::explain(id);
                }
            }
            (Kind::Permission, Button::Allow | Button::Deny) => {
                let Some(pane_id) = pane_of(key) else { return };
                let shown = slot.page.borrow().prompts.get(key).cloned();
                let choice = if button == Button::Allow {
                    crate::prompt_preview::PermissionChoice::Approve
                } else {
                    crate::prompt_preview::PermissionChoice::Reject
                };
                // Only onto the prompt the card showed: a screen that moved
                // on is left alone, the next render shows what is there now.
                let typed = super::sidebar_ui::with_pane(pane_id, |p| match p.permission_prompt() {
                    Some(prompt) if shown.as_deref() == Some(crate::chat::cards::permission_id(&prompt).as_str()) => {
                        p.answer_permission(&prompt, choice)
                    }
                    _ => false,
                })
                .unwrap_or(false);
                if typed {
                    slot.page.borrow_mut().pending.insert(key.to_string(), now_ms());
                }
            }
            _ => {}
        }
        self.mark_dirty();
    }

    /// An instruction typed on an opened attention card: `act_command ID
    /// instruct --text=<text>`, which hands it to the chief of the card's
    /// scope. Nothing typed sends nothing.
    fn cockpit_instruct(&self, key: &str, text: &str) {
        let Some(id) = key.strip_prefix("attention:") else { return };
        if text.trim().is_empty() {
            return;
        }
        if let Err(why) = crate::attention::perform_with(id, Verb::Instruct, Some(text)) {
            log::warn!("cockpit: {why}");
            crate::attention::instruction_refused(id, text, "Kova n'a pas pu la transmettre");
        }
        self.mark_dirty();
    }

    /// What the tick adds to the live cards: their scope from the cwd
    /// (`[attention] perso_dir`), and the detail of the cards opened.
    fn cockpit_fill(&self, items: &mut [Item]) {
        let slot = &self.ivars().cockpit;
        let perso = self.ivars().config.get().and_then(|c| c.attention.perso_root());
        let routines = crate::routines::snapshot();
        let routines = routines.as_deref().map(Vec::as_slice);
        let entries = crate::attention::snapshot();
        slot.expanded.borrow_mut().retain(|k| items.iter().any(|i| &i.key == k));
        let expanded = slot.expanded.borrow();
        for item in items.iter_mut() {
            let routine = find_routine(routines, &item.key);
            let cwd = match item.kind {
                Kind::FailedRoutine => routine.map(|r| r.cwd.clone()),
                Kind::Permission | Kind::Error => {
                    pane_of(&item.key).and_then(|id| super::sidebar_ui::with_pane(id, |p| p.cached_cwd()).flatten())
                }
                _ => None,
            };
            if let Some(cwd) = cwd {
                item.scope = cockpit::scope_of_cwd(&cwd, perso.as_deref());
            }
            // A run, a pause or a resume in flight: the card waits for it.
            if let Some(r) = routine
                && self.ivars().sidebar.borrow().sched.busy_action(&r.name).is_some()
            {
                item.state = State::Running;
            }
            if expanded.contains(&item.key) {
                item.detail = match item.key.strip_prefix("attention:") {
                    Some(id) => entries
                        .as_deref()
                        .and_then(|list| list.iter().find(|e| e.id == id))
                        .map(|e| cockpit::entry_detail(e, crate::attention::explaining(id))),
                    None => self.cockpit_detail(item, routine),
                };
            }
        }
    }

    /// What an opened card shows: the Scheduled tile's body for a routine,
    /// the prompt for a permission, the pane's last lines for an error, the
    /// question and its answers for an ask.
    fn cockpit_detail(&self, item: &Item, routine: Option<&Routine>) -> Option<Detail> {
        use super::sidebar_sched::{self, SchedButton};
        match item.kind {
            Kind::FailedRoutine => {
                let r = routine?;
                let vm = super::sidebar_model::RoutineVm::of(r, true);
                let labels = ["Horaire", "Prochain", "Dernier", "Dossier"];
                let rows = labels.iter().zip(sidebar_sched::body_rows(&vm)).map(|(l, (_, v))| (l.to_string(), v)).collect();
                let prompt = std::fs::read_to_string(crate::routines::prompt_path(&r.name))
                    .map(|mut text| {
                        if text.len() > PROMPT_MAX {
                            let mut cut = PROMPT_MAX;
                            while !text.is_char_boundary(cut) {
                                cut -= 1;
                            }
                            text.truncate(cut);
                            text.push('\u{2026}');
                        }
                        text
                    })
                    .unwrap_or_else(|_| "Prompt introuvable".to_string());
                let acts = sidebar_sched::buttons(&vm)
                    .iter()
                    .filter_map(|b| match b {
                        SchedButton::Run => Some(RoutineAct::Run),
                        SchedButton::Resume => Some(RoutineAct::Resume),
                        SchedButton::Log => Some(RoutineAct::Log),
                        SchedButton::Pause => Some(RoutineAct::Pause),
                        SchedButton::Edit => Some(RoutineAct::Edit),
                        _ => None,
                    })
                    .collect();
                let note = self.ivars().sidebar.borrow().sched.error(&r.name).map(str::to_string);
                let failed = matches!(vm.last_status, crate::routines::LastStatus::Failed(_));
                Some(Detail::Routine { rows, failed, prompt, acts, note })
            }
            Kind::Permission => {
                let prompt = super::sidebar_ui::with_pane(pane_of(&item.key)?, |p| p.permission_prompt()).flatten()?;
                let mut text = prompt.lines.join("\n");
                if !prompt.question.is_empty() {
                    text.push_str("\n\n");
                    text.push_str(&prompt.question);
                }
                Some(Detail::Permission { text })
            }
            Kind::Error => {
                let visible = super::sidebar_ui::with_pane(pane_of(&item.key)?, |p| {
                    p.terminal.read().dump_text(crate::terminal::DumpMode::Visible, true).text
                })?;
                Some(Detail::Error { words: item.needs.clone(), lines: cockpit::last_lines(&visible, ERROR_LINES) })
            }
            // A sheet's card shows its decisions; its button opens it.
            Kind::Ask | Kind::Sheet => {
                let q = self.waiting_ask(item.key.strip_prefix("ask:")?)?;
                let card = crate::chat::cards::question_card(&q, crate::handoff::detail(&q).map(|d| d.card_text()));
                let message = match &card.detail {
                    Some(d) => format!("{}\n\n{d}", card.message),
                    None => card.message.clone(),
                };
                let answers = card
                    .buttons
                    .iter()
                    .filter(|b| matches!(b.act, crate::chat::cards::Act::Answer { .. }))
                    .map(|b| (b.label.clone(), b.needs_text))
                    .collect();
                Some(Detail::Ask { message, answers, input: card.input })
            }
            _ => None,
        }
    }

    /// A waiting `kova ask`, by id.
    fn waiting_ask(&self, id: &str) -> Option<crate::ask::Question> {
        let mut found = None;
        let mtm = MainThreadMarker::from(self);
        crate::app::app_delegate(mtm).ivars().for_each_waiting_ask(|q| {
            if q.id == id {
                found = Some(q.clone());
            }
        });
        found
    }

    /// An opened routine card's button: the Scheduled tile's own path.
    fn cockpit_routine(&self, key: &str, word: &str) {
        use super::sidebar_sched::{SchedButton, SchedHit};
        let (Some(name), Some(act)) = (key.strip_prefix("routine:"), RoutineAct::parse(word)) else { return };
        let button = match act {
            RoutineAct::Run => SchedButton::Run,
            RoutineAct::Resume => SchedButton::Resume,
            RoutineAct::Log => SchedButton::Log,
            RoutineAct::Pause => SchedButton::Pause,
            RoutineAct::Edit => SchedButton::Edit,
        };
        let routines = crate::routines::snapshot();
        let log = find_routine(routines.as_deref().map(Vec::as_slice), key).map(|r| r.log.clone()).unwrap_or_default();
        // The index names a row of the sidebar's list; the action ignores it.
        self.sidebar_sched_hit(SchedHit::Button(0, button), name, &log);
        self.mark_dirty();
    }

    /// A link of an opened attention card, in the side panel like `Ouvrir`:
    /// only one the card shows.
    fn cockpit_link(&self, key: &str, link: &str) {
        let shown = self.ivars().cockpit.page.borrow().items.iter().any(|i| {
            i.key == key && matches!(&i.detail, Some(Detail::Entry { links, .. }) if links.iter().any(|l| l == link))
        });
        if !shown {
            return;
        }
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let exists = |p: &Path| p.is_file();
        if let Some(target) = super::sidebar_attention::open_target(link, home.as_deref(), &exists) {
            self.open_in_side_panel(target);
        }
        self.mark_dirty();
    }

    /// An opened question's answer, checked and queued as the chat does.
    fn cockpit_answer(&self, key: &str, action: &str, text: &str) {
        let Some(q) = key.strip_prefix("ask:").and_then(|id| self.waiting_ask(id)) else { return };
        let card = crate::chat::cards::question_card(&q, None);
        let checked = if crate::chat::cards::offers(&card, action) {
            crate::ask::accepts_external_answer(&q, action, text)
        } else {
            Err("this answer is not offered".to_string())
        };
        match checked {
            Ok(()) => {
                let mtm = MainThreadMarker::from(self);
                crate::app::app_delegate(mtm).ivars().queue_ask_action(super::sidebar_asks::AskAction::Answer {
                    id: q.id.clone(),
                    action: action.to_string(),
                    text: text.to_string(),
                });
                self.ivars().cockpit.page.borrow_mut().pending.insert(key.to_string(), now_ms());
            }
            Err(why) => log::warn!("cockpit: {why}"),
        }
        self.mark_dirty();
    }

    fn cockpit_web_view(&self, mtm: MainThreadMarker) -> Retained<WKWebView> {
        let slot = &self.ivars().cockpit;
        if let Some(web) = slot.web.get() {
            return web.clone();
        }
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        // Nothing to keep: no cookie, no storage, nothing on disk.
        unsafe { config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm)) };
        let delegate: Retained<CockpitWebDelegate> = {
            let this = CockpitWebDelegate::alloc(mtm).set_ivars(CockpitDelegateIvars::default());
            unsafe { msg_send![super(this), init] }
        };
        unsafe {
            config
                .userContentController()
                .addScriptMessageHandler_name(ProtocolObject::from_ref(&*delegate), &NSString::from_str(cockpit::HANDLER));
        }
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 100.0, height: 100.0 } };
        let web = unsafe { WKWebView::initWithFrame_configuration(mtm.alloc(), frame, &config) };
        unsafe {
            web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            let no: Retained<NSNumber> = NSNumber::new_bool(false);
            let _: () = msg_send![&web, setValue: &*no, forKey: &*NSString::from_str("drawsBackground")];
            web.setInspectable(true);
        }
        web.setHidden(true);
        if let Some(container) = unsafe { self.superview() } {
            container.addSubview_positioned_relativeTo(&web, NSWindowOrderingMode::Above, Some(self));
            // The chat's web view, if it was made first, goes back above the
            // page: the chief's chat is drawn over its right third.
            if let Some(chat) = self.chat_web() {
                container.addSubview_positioned_relativeTo(&chat, NSWindowOrderingMode::Above, Some(&web));
            }
        }
        let _ = slot.delegate.set(delegate);
        let _ = slot.web.set(web.clone());
        web
    }
}

/// The routine a `routine:<name>` key names.
fn find_routine<'a>(list: Option<&'a [Routine]>, key: &str) -> Option<&'a Routine> {
    let name = key.strip_prefix("routine:")?;
    list?.iter().find(|r| r.name == name)
}

/// The pane a `pane:<id>` key names.
fn pane_of(key: &str) -> Option<PaneId> {
    key.strip_prefix("pane:")?.parse::<u64>().ok().and_then(|id| PaneId::try_from(id).ok())
}

#[derive(Default)]
pub struct CockpitDelegateIvars {
    inbox: RefCell<Vec<String>>,
    crashed: Cell<bool>,
}

impl CockpitDelegateIvars {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.inbox.borrow_mut())
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KovaCockpitWebDelegate"]
    #[ivars = CockpitDelegateIvars]
    pub struct CockpitWebDelegate;

    unsafe impl NSObjectProtocol for CockpitWebDelegate {}

    unsafe impl WKNavigationDelegate for CockpitWebDelegate {
        /// The page Kova loads, and nothing else: a clicked http(s) link
        /// opens in the default browser, every other navigation is cancelled.
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_policy(
            &self,
            _web_view: &WKWebView,
            action: &WKNavigationAction,
            handler: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let url = unsafe { action.request() }.URL();
            let text = url.as_deref().and_then(|u| u.absoluteString()).map(|s| s.to_string()).unwrap_or_default();
            let clicked = unsafe { action.navigationType() } == WKNavigationType::LinkActivated;
            let lower = text.to_ascii_lowercase();
            let policy = if !clicked && (text.is_empty() || lower == "about:blank") {
                WKNavigationActionPolicy::Allow
            } else {
                if clicked && (lower.starts_with("https://") || lower.starts_with("http://"))
                    && let Some(u) = url.as_deref()
                {
                    NSWorkspace::sharedWorkspace().openURL(u);
                }
                WKNavigationActionPolicy::Cancel
            };
            handler.call((policy,));
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn content_process_did_terminate(&self, _web_view: &WKWebView) {
            log::warn!("cockpit: the page's process ended, loading it again");
            self.ivars().crashed.set(true);
        }
    }

    unsafe impl WKScriptMessageHandler for CockpitWebDelegate {
        /// What the page posts: taken only from the main frame of a page on
        /// `about:blank`, the one Kova loads.
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive_script_message(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            let frame = unsafe { message.frameInfo() };
            let frame_url = unsafe { frame.request() }.URL().and_then(|u| u.absoluteString()).map(|s| s.to_string());
            let page_url = unsafe { message.webView() }
                .and_then(|w| unsafe { w.URL() })
                .and_then(|u| u.absoluteString())
                .map(|s| s.to_string());
            let blank = |u: Option<&str>| u.is_none_or(|u| u.is_empty() || u == "about:blank");
            if !unsafe { frame.isMainFrame() } || !blank(frame_url.as_deref()) || !blank(page_url.as_deref()) {
                log::warn!("cockpit: ignored a message from {:?}", frame_url);
                return;
            }
            let body = unsafe { message.body() };
            let Some(text) = body.downcast_ref::<NSString>().map(|s| s.to_string()) else { return };
            if text.len() <= MESSAGE_MAX {
                self.ivars().inbox.borrow_mut().push(text);
            }
        }
    }
);
