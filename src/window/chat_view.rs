//! Chat mode on the window's side: a web view laid over the pane that shows
//! its conversation as a chat (`docs/chat-mode.md`, the pure half in
//! `crate::chat`).
//!
//! One web view per window, created on first use and shared: it sits over
//! the active tab's chat pane (at most one per tab), its grid and PTY going
//! on underneath, and hides when there is none, when the pane is minimized,
//! or while an overlay Kova draws in Metal (the switcher, the search palette,
//! help, a rename) needs the window. The pane's status bar stays visible.
//!
//! The page posts what the user does (`kovaChat` messages); the tick reads
//! the transcript and the pane, answers through the existing paths and
//! pushes only what changed (`chat::page::Update`). A message is taken only
//! from this web view's main frame on `about:blank`, the page Kova loads.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;

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

use crate::chat::{banner, cards, conversation::Conversation, input, page, tail::Tail};
use crate::pane::Pane;
use crate::surface::PaneId;

use super::sidebar::tokens;
use super::KovaView;

/// How often the transcript, the cards and the gate are read again.
const POLL_MS: u128 = 200;
/// A page message longer than this is not one the page sends.
const MESSAGE_MAX: usize = 64 * 1024;

/// The window's chat: its web view and what it shows.
#[derive(Default)]
pub struct ChatSlot {
    web: OnceCell<Retained<WKWebView>>,
    delegate: OnceCell<Retained<ChatWebDelegate>>,
    state: RefCell<Option<ChatState>>,
    /// The web view had the keyboard at the last tick.
    had_keyboard: Cell<bool>,
    /// The frame last given to the web view, in the container's coordinates.
    frame: Cell<Option<(f64, f64, f64, f64)>>,
    /// The theme the page was loaded in (`theme::generation`).
    theme: Cell<u64>,
}

/// The chat of one pane, from the moment it is shown.
struct ChatState {
    pane_id: PaneId,
    session: Option<String>,
    tail: Option<Tail>,
    conv: Conversation,
    exchanges: usize,
    /// What the page holds: entry id -> hash, card id -> hash.
    sent_entries: HashMap<String, u64>,
    sent_cards: HashMap<String, u64>,
    /// The pane's questions and prompt as last read: what a click answers.
    questions: Vec<crate::ask::Question>,
    cards: Vec<cards::Card>,
    permission: Option<crate::prompt_preview::ParsedPrompt>,
    shown: Option<(u64, bool, usize)>,
    shown_cards: Option<Vec<(String, u64)>>,
    shown_banner: Option<page::BannerWire>,
    shown_queued: Option<Option<page::Queued>>,
    working_since: Option<u64>,
    /// The page posted `ready`: updates reach it.
    ready: bool,
    first: bool,
    focus: bool,
    outbox: input::Outbox,
    queued_reason: &'static str,
    note: Option<String>,
    restore: Option<String>,
    /// The images pasted into the box, by their files: only these go with a message.
    pasted: Vec<String>,
    attach: Vec<page::Attach>,
    /// Dictation: its thread while it runs, whether it listens, what it heard.
    voice: Option<crate::voice_io::Session>,
    listening: bool,
    transcript: crate::voice::Transcript,
    shown_voice: Option<page::Voice>,
    last_poll: Option<std::time::Instant>,
    /// The nonce of the page loaded for this pane: a message carrying
    /// another one was posted by an earlier load.
    load: String,
    /// The last lookup of a transcript not written yet (a scan of every
    /// project folder when the cwd is unknown): once a second at most.
    looked: Option<std::time::Instant>,
}

impl ChatState {
    fn new(pane_id: PaneId, load: String) -> Self {
        ChatState {
            pane_id,
            session: None,
            tail: None,
            conv: Conversation::new(),
            exchanges: page::EXCHANGES_STEP,
            sent_entries: HashMap::new(),
            sent_cards: HashMap::new(),
            questions: Vec::new(),
            cards: Vec::new(),
            permission: None,
            shown: None,
            shown_cards: None,
            shown_banner: None,
            shown_queued: None,
            working_since: None,
            ready: false,
            first: true,
            focus: false,
            outbox: input::Outbox::default(),
            queued_reason: "",
            note: None,
            restore: None,
            pasted: Vec::new(),
            attach: Vec::new(),
            voice: None,
            listening: false,
            transcript: crate::voice::Transcript::default(),
            shown_voice: None,
            last_poll: None,
            load,
            looked: None,
        }
    }

    /// Forget what the page holds: the next update carries everything.
    fn resync(&mut self) {
        self.sent_entries.clear();
        self.sent_cards.clear();
        self.shown = None;
        self.shown_cards = None;
        self.shown_banner = None;
        self.shown_queued = None;
        self.shown_voice = None;
    }

    /// Dictation on, or off: off, what was said still arrives.
    fn toggle_voice(&mut self) {
        if self.listening {
            self.listening = false;
            if let Some(session) = &self.voice {
                session.stop();
            }
        } else {
            self.voice = Some(crate::voice_io::Session::start());
            self.listening = true;
            self.transcript = crate::voice::Transcript::default();
        }
    }
}

fn css_hex(c: [f32; 3]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

/// The page's colours: the design system's, light or dark as the terminal is.
fn theme() -> page::Theme {
    let p = tokens::get();
    page::Theme {
        dark: p != &tokens::LIGHT,
        bg: css_hex(p.ground),
        bg2: css_hex(p.bg2),
        t1: css_hex(p.text_primary),
        t2: css_hex(p.text_secondary),
        t3: css_hex(p.text_tertiary),
        b2: css_hex(p.border_strong),
        b3: css_hex(p.border_subtle),
        accent: css_hex(p.sel_rail),
        working: css_hex(p.working),
        awaiting: css_hex(p.awaiting),
        error: css_hex(p.error),
        ok: css_hex(p.ok_dot),
    }
}

/// What the tick found about the chat pane, read under the tabs' borrow.
struct Target {
    pane_id: PaneId,
    /// In the container's coordinates (points, y up).
    frame: (f64, f64, f64, f64),
    focused: PaneId,
    /// The chief's chat on the Cockpit page: not this tab's chat pane.
    cockpit: bool,
}

impl KovaView {
    /// `toggle-chat`: the focused pane as a chat, or as its terminal again.
    /// A pane without a Claude conversation stays a terminal, with a note.
    /// One chat pane per tab: turning one on turns the others off.
    pub(super) fn toggle_chat(&self) {
        let note = {
            let mut tabs = self.ivars().tabs.borrow_mut();
            let Some(tab) = tabs.get_mut(self.ivars().active_tab.get()) else { return };
            let focused = tab.focused_pane;
            let Some(pane) = tab.pane(focused) else { return };
            if pane.chat {
                if let Some(p) = tab.pane_mut(focused) {
                    p.chat = false;
                }
                None
            } else if pane.last_claude_session_id().is_none() {
                Some("No Claude conversation in this pane")
            } else {
                let mut ids = Vec::new();
                tab.for_each_pane(&mut |p| ids.push(p.id));
                for id in ids {
                    if let Some(p) = tab.pane_mut(id) {
                        p.chat = id == focused;
                    }
                }
                None
            }
        };
        if let Some(note) = note {
            self.set_transient_status(note);
        }
        self.mark_dirty();
    }

    /// Whether `responder` (the window's first responder) is the chat's web view.
    pub(super) fn chat_holds(&self, responder: &objc2::runtime::AnyObject) -> bool {
        let (Some(web), Some(view)) = (self.ivars().chat.web.get(), responder.downcast_ref::<NSView>()) else { return false };
        view.isDescendantOf(web)
    }

    /// An overlay Kova draws over the panes is open: the chat steps aside.
    pub(super) fn metal_overlay_open(&self) -> bool {
        let i = self.ivars();
        i.search_palette.borrow().is_some()
            || i.pane_switcher.borrow().is_some()
            || i.filter.borrow().is_some()
            || i.recent_projects.borrow().is_some()
            || i.send_to_window.borrow().is_some()
            || i.merge_tab.borrow().is_some()
            || i.rename_pane.borrow().is_some()
            || i.rename_tab.borrow().is_some()
            || i.show_help.get()
            || i.show_mem_report.get()
    }

    /// The active tab's chat pane, where it is on screen, and the focused pane.
    /// While the Cockpit is open it covers the panes: the only chat is the
    /// chief's, on the page's left third, and its pane counts as focused so
    /// nothing moves the focus or switches tabs to it.
    fn chat_target(&self) -> Option<Target> {
        if self.metal_overlay_open() {
            return None;
        }
        if self.cockpit_open() {
            return self
                .cockpit_chat_target()
                .map(|(pane_id, frame)| Target { pane_id, frame, focused: pane_id, cockpit: true });
        }
        let tabs = self.ivars().tabs.borrow();
        let tab = tabs.get(self.ivars().active_tab.get())?;
        let mut found = None;
        tab.for_each_pane(&mut |p| {
            if p.chat && found.is_none() && !p.minimized {
                found = Some(p.id);
            }
        });
        let pane_id = found?;
        let vp = tab.viewport_for_pane(pane_id, self.panes_viewport_for_tab(tab))?;
        let scale = self.backing_scale().max(1.0) as f64;
        let renderer = self.ivars().renderer.get()?;
        let (bar, cell_h) = {
            let r = renderer.read();
            (r.status_bar_enabled(), r.cell_size().1)
        };
        // Points, from the view's top-left; the status bar row stays Kova's.
        let bar_h = if bar { cell_h as f64 } else { 0.0 };
        let view = self.frame();
        let x0 = (vp.x as f64 / scale).max(0.0);
        let x1 = ((vp.x + vp.width) as f64 / scale).min(view.size.width);
        let top = vp.y as f64 / scale;
        let h = ((vp.height as f64 - bar_h) / scale).max(0.0);
        if x1 - x0 < 40.0 || h < 40.0 {
            return None;
        }
        // The view's y axis points up; the container holds it at its origin.
        let y = view.size.height - top - h;
        Some(Target {
            pane_id,
            frame: (view.origin.x + x0, view.origin.y + y, x1 - x0, h),
            focused: tab.focused_pane,
            cockpit: false,
        })
    }

    /// Once per frame: place the web view over the chat pane (or hide it),
    /// keep the keyboard where the focus is, serve the page's messages and
    /// push what changed.
    pub(super) fn tick_chat(&self) {
        let Some(window) = self.window() else { return };
        let slot = &self.ivars().chat;
        let target = self.chat_target();
        let Some(target) = target else {
            if let Some(web) = slot.web.get()
                && !web.isHidden()
            {
                if window.firstResponder().is_some_and(|r| self.chat_holds(&r)) {
                    window.makeFirstResponder(Some(self));
                }
                web.setHidden(true);
            }
            // A pane that left chat mode drops its state; one hidden by an
            // overlay keeps it.
            let gone = slot.state.borrow().as_ref().is_some_and(|st| !self.pane_is_chat(st.pane_id));
            // Out of sight, no one sees what the mic hears: it stops.
            if let Some(st) = slot.state.borrow_mut().as_mut()
                && st.listening
            {
                st.toggle_voice();
            }
            if gone {
                slot.state.replace(None);
                if let Some(web) = slot.web.get() {
                    unsafe { web.loadHTMLString_baseURL(&NSString::from_str(""), None) };
                }
            }
            slot.had_keyboard.set(false);
            return;
        };
        let mtm = MainThreadMarker::from(self);
        let web = self.chat_web_view(mtm);
        if slot.delegate.get().is_some_and(|d| d.ivars().crashed.take()) {
            slot.state.replace(None);
        }
        let generation = crate::theme::generation();
        let fresh = slot.state.borrow().as_ref().is_none_or(|st| st.pane_id != target.pane_id);
        if fresh {
            slot.theme.set(generation);
            let nonce = crate::ask::new_question_id().replace('-', "");
            slot.state.replace(Some(ChatState::new(target.pane_id, nonce.clone())));
            let title = self.with_pane(target.pane_id, |p| p.display_title("Chat")).unwrap_or_default();
            let html = page::shell(&theme(), &title, &nonce);
            unsafe { web.loadHTMLString_baseURL(&NSString::from_str(&html), None) };
        }
        // The theme switched: the colours change in place, the page keeps
        // its input, its scroll and what it shows.
        if slot.theme.get() != generation {
            slot.theme.set(generation);
            unsafe { web.evaluateJavaScript_completionHandler(&NSString::from_str(&page::restyle_script(&theme())), None) };
        }
        if slot.frame.get() != Some(target.frame) {
            let (x, y, w, h) = target.frame;
            web.setFrame(CGRect { origin: CGPoint { x, y }, size: CGSize { width: w, height: h } });
            slot.frame.set(Some(target.frame));
        }
        if web.isHidden() {
            web.setHidden(false);
        }

        // The keyboard follows the focus: a click into the chat focuses its
        // pane, the chat pane focused gets the box, another pane focused
        // takes the keyboard back.
        let responder = window.firstResponder();
        let in_chat = responder.as_ref().is_some_and(|r| self.chat_holds(r));
        let on_terminal = responder.as_ref().is_some_and(|r| std::ptr::eq(Retained::as_ptr(r).cast::<()>(), (self as *const Self).cast::<()>()));
        if in_chat && !slot.had_keyboard.get() && target.focused != target.pane_id {
            self.focus_pane_in_window(target.pane_id);
        } else if target.focused == target.pane_id && (on_terminal || fresh) {
            window.makeFirstResponder(Some(&web));
            if let Some(st) = slot.state.borrow_mut().as_mut() {
                st.focus = true;
            }
        } else if target.focused != target.pane_id && in_chat {
            window.makeFirstResponder(Some(self));
        }
        slot.had_keyboard.set(window.firstResponder().is_some_and(|r| self.chat_holds(&r)));

        // The page's messages, then the poll.
        let messages = slot.delegate.get().map(|d| d.ivars().take()).unwrap_or_default();
        let mut leave = false;
        let mut update = None;
        {
            let tabs = self.ivars().tabs.borrow();
            let Some(pane) = tabs.iter().find_map(|t| t.pane(target.pane_id)) else { return };
            let mut state = slot.state.borrow_mut();
            let Some(st) = state.as_mut() else { return };
            let mut force = false;
            for raw in messages {
                match self.chat_message(st, pane, &raw) {
                    Reply::Leave => leave = true,
                    Reply::Push => force = true,
                    Reply::Nothing => {}
                }
            }
            let due = st.last_poll.is_none_or(|t| t.elapsed().as_millis() >= POLL_MS);
            if due || force {
                st.last_poll = Some(std::time::Instant::now());
                update = chat_poll(st, pane);
            }
        }
        if let Some(update) = update {
            unsafe { web.evaluateJavaScript_completionHandler(&NSString::from_str(&page::update_script(&update)), None) };
        }
        if leave && target.cockpit {
            self.cockpit_close_chat();
        } else if leave {
            let mut tabs = self.ivars().tabs.borrow_mut();
            if let Some(p) = tabs.iter_mut().find_map(|t| t.pane_mut(target.pane_id)) {
                p.chat = false;
            }
        }
    }

    fn pane_is_chat(&self, id: PaneId) -> bool {
        self.with_pane(id, |p| p.chat).unwrap_or(false)
    }

    fn with_pane<T>(&self, id: PaneId, f: impl FnOnce(&Pane) -> T) -> Option<T> {
        let tabs = self.ivars().tabs.borrow();
        tabs.iter().find_map(|t| t.pane(id)).map(f)
    }

    /// One message from the page.
    fn chat_message(&self, st: &mut ChatState, pane: &Pane, raw: &str) -> Reply {
        let Ok(msg) = serde_json::from_str::<Value>(raw) else { return Reply::Nothing };
        let s = |k: &str| msg.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        if s("load") != st.load {
            return Reply::Nothing;
        }
        match msg.get("type").and_then(Value::as_str).unwrap_or_default() {
            "ready" => {
                st.ready = true;
                st.first = true;
                st.resync();
                Reply::Push
            }
            "resync" => {
                st.resync();
                Reply::Push
            }
            "more" => {
                st.exchanges += page::EXCHANGES_STEP;
                Reply::Push
            }
            "terminal" => Reply::Leave,
            "cancel" => {
                st.restore = st.outbox.cancel();
                Reply::Push
            }
            "voice" => {
                st.toggle_voice();
                Reply::Push
            }
            "voice-stop" => {
                if st.listening {
                    st.toggle_voice();
                }
                Reply::Push
            }
            "paste-image" => {
                if !attach_pasted_image(st) {
                    st.note = Some("Pas d'image dans le presse-papier".to_string());
                }
                Reply::Push
            }
            "link" => {
                let href = s("href");
                if href.to_ascii_lowercase().starts_with("mailto:") {
                    if let Some(url) = objc2_foundation::NSURL::URLWithString(&NSString::from_str(&href)) {
                        NSWorkspace::sharedWorkspace().openURL(&url);
                    }
                } else {
                    let cwd = pane.cached_cwd().map(std::path::PathBuf::from);
                    match super::side_panel::target_on_disk(&href, cwd.as_deref()) {
                        Some(target) => self.open_in_side_panel(target),
                        None if crate::chat::links::is_web(&href) => st.note = Some("Lien non pris en charge".to_string()),
                        None => st.note = Some("Fichier introuvable".to_string()),
                    }
                }
                Reply::Push
            }
            "send" => {
                let images: Vec<String> = msg
                    .get("images")
                    .and_then(Value::as_array)
                    .map(|list| list.iter().filter_map(Value::as_str).filter(|p| st.pasted.iter().any(|q| q == p)).map(str::to_string).collect())
                    .unwrap_or_default();
                // What was dictated went with it: dictation goes on from here.
                if st.listening {
                    st.transcript.clear();
                }
                let text = input::with_images(&s("text"), &images);
                match input::sanitize(&text) {
                    Err(reason) => {
                        st.note = Some(reason.to_string());
                        st.restore = Some(text);
                    }
                    Ok(text) => match st.outbox.submit(text, gate(pane), crate::surface::monotonic_ms()) {
                        input::Step::Type(text) => type_into(pane, &text),
                        input::Step::Queued(reason) => st.queued_reason = reason,
                        input::Step::Refused { reason, text } => {
                            st.note = Some(reason.to_string());
                            st.restore = Some(text);
                        }
                        input::Step::Enter | input::Step::Stale | input::Step::Nothing => {}
                    },
                }
                Reply::Push
            }
            "answer" => {
                let (id, action, text) = (s("id"), s("action"), s("text"));
                let question = st.questions.iter().find(|q| q.id == id);
                let card = st.cards.iter().find(|c| c.id == id);
                let checked = match (question, card) {
                    (Some(q), Some(c)) if cards::offers(c, &action) => crate::ask::accepts_external_answer(q, &action, &text),
                    _ => Err("this question is no longer waiting".to_string()),
                };
                // The page disabled the card's buttons. They come back when
                // the answer is refused, or after a handoff's step (the
                // question waits for Fait); an answer taken leaves them off.
                let again = match checked {
                    Ok(()) => {
                        let step = question.is_some_and(|q| crate::handoff::detail(q).is_some());
                        let mtm = MainThreadMarker::from(self);
                        crate::app::app_delegate(mtm)
                            .ivars()
                            .queue_ask_action(super::sidebar_asks::AskAction::Answer { id: id.clone(), action, text });
                        step
                    }
                    Err(e) => {
                        st.note = Some(e);
                        true
                    }
                };
                if again {
                    st.sent_cards.remove(&id);
                    st.shown_cards = None;
                }
                Reply::Push
            }
            "open" => {
                let id = s("id");
                match st.questions.iter().find(|q| q.id == id).and_then(|q| q.panel_link()) {
                    Some(link) => crate::ask_card::open_in_panel(id, link),
                    None => st.note = Some("This decision is no longer waiting".to_string()),
                }
                Reply::Push
            }
            "permission" => {
                let id = s("id");
                let choice = serde_json::from_value::<cards::Choice>(msg.get("choice").cloned().unwrap_or(Value::Null)).ok();
                let typed = match (&st.permission, choice) {
                    (Some(prompt), Some(choice)) if cards::permission_id(prompt) == id => pane.answer_permission(prompt, choice.into()),
                    _ => false,
                };
                // Typed: the card stays off until the prompt leaves the screen.
                if !typed {
                    st.note = Some("The prompt changed: look at the terminal".to_string());
                    st.sent_cards.remove(&id);
                    st.shown_cards = None;
                }
                Reply::Push
            }
            _ => Reply::Nothing,
        }
    }

    /// `toggle-voice`: dictation in the chat shown, on or off.
    pub(super) fn toggle_voice(&self) {
        let toggled = match self.ivars().chat.state.borrow_mut().as_mut() {
            Some(st) if st.ready => {
                st.toggle_voice();
                st.last_poll = None;
                true
            }
            _ => false,
        };
        if !toggled {
            self.set_transient_status("La dictée marche dans le chat");
        }
    }

    /// Cmd+V or Ctrl+V in the chat's box with an image on the pasteboard:
    /// the image goes into the box, as in the terminal. False without one,
    /// so the box pastes text as usual.
    pub(super) fn chat_paste_image(&self) -> bool {
        let mut state = self.ivars().chat.state.borrow_mut();
        let Some(st) = state.as_mut() else { return false };
        let done = attach_pasted_image(st);
        if done {
            st.last_poll = None;
        }
        done
    }

    /// The chat's web view, once made: the Cockpit puts it back above its page.
    pub(super) fn chat_web(&self) -> Option<Retained<WKWebView>> {
        self.ivars().chat.web.get().cloned()
    }

    fn chat_web_view(&self, mtm: MainThreadMarker) -> Retained<WKWebView> {
        let slot = &self.ivars().chat;
        if let Some(web) = slot.web.get() {
            return web.clone();
        }
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        // Nothing to keep: no cookie, no storage, nothing on disk.
        unsafe { config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm)) };
        let delegate: Retained<ChatWebDelegate> = {
            let this = ChatWebDelegate::alloc(mtm).set_ivars(ChatDelegateIvars::default());
            unsafe { msg_send![super(this), init] }
        };
        unsafe {
            config
                .userContentController()
                .addScriptMessageHandler_name(ProtocolObject::from_ref(&*delegate), &NSString::from_str(page::HANDLER));
        }
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 100.0, height: 100.0 } };
        let web = unsafe { WKWebView::initWithFrame_configuration(mtm.alloc(), frame, &config) };
        unsafe {
            web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            // No white flash before the page paints its own ground.
            let no: Retained<NSNumber> = NSNumber::new_bool(false);
            let _: () = msg_send![&web, setValue: &*no, forKey: &*NSString::from_str("drawsBackground")];
            web.setInspectable(true);
        }
        web.setHidden(true);
        if let Some(container) = unsafe { self.superview() } {
            // Above the Cockpit's page when there is one: the chief's chat
            // is drawn over its left third.
            match self.cockpit_web() {
                Some(page) => container.addSubview_positioned_relativeTo(&web, NSWindowOrderingMode::Above, Some(&page)),
                None => container.addSubview_positioned_relativeTo(&web, NSWindowOrderingMode::Above, Some(self)),
            }
        }
        let _ = slot.delegate.set(delegate);
        let _ = slot.web.set(web.clone());
        web
    }
}

enum Reply {
    Nothing,
    /// Something changed: push an update now.
    Push,
    /// Back to the terminal.
    Leave,
}

/// Paste `text` into the pane, bracketed when the app asked for it.
fn type_into(pane: &Pane, text: &str) {
    let bracketed = pane.terminal.read().bracketed_paste;
    log::info!("pane {}: chat typed {} bytes", pane.id, text.len());
    pane.paste(&input::typed(text, bracketed));
}

/// Whether the pane's prompt takes a message now (`chat::input::gate`).
fn gate(pane: &Pane) -> input::Verdict {
    let visible = pane.terminal.read().dump_text(crate::terminal::DumpMode::Visible, true).text;
    input::gate(input::GateInput {
        // The session id lags Claude's exit; a fresh probe of the foreground
        // process does not, so a shell's own `❯` never reads as Claude's.
        has_session: pane.claude_session_id().is_some() && pane.pty.foreground_process().is_some(),
        working: pane.is_working(),
        permission_prompt: pane.has_permission_prompt() || crate::prompt_preview::parse_permission_prompt(&visible).is_some(),
        composer: input::composer(&visible),
    })
}

/// The pasteboard's image written where a terminal paste writes it
/// (`appkit_input::pasted_image_path`), shown in the box, sent with the next
/// message. False without an image.
fn attach_pasted_image(st: &mut ChatState) -> bool {
    let Some(path) = crate::appkit_input::pasted_image_path(&objc2_app_kit::NSPasteboard::generalPasteboard()) else { return false };
    st.attach.push(page::Attach { src: thumbnail(&path), path: path.clone() });
    st.pasted.push(path);
    true
}

/// A pasted image as a `data:` URL for its chip, when small enough to send to the page.
fn thumbnail(path: &str) -> Option<String> {
    const MAX: u64 = 3 * 1024 * 1024;
    if std::fs::metadata(path).ok()?.len() > MAX {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    Some(format!("data:image/png;base64,{}", crate::decision_sheet::base64(&bytes)))
}

/// The note when the Enter after a paste was not written.
const NOT_ENTERED: &str = "Not sent: the text is in the terminal's prompt, press Enter there";

/// Read the transcript, the cards and the gate again; the update to push,
/// if the page is ready and something changed.
fn chat_poll(st: &mut ChatState, pane: &Pane) -> Option<page::Update> {
    let now = crate::ask::now_epoch();
    // The conversation: a new id (a `/clear`, a resume) is another transcript.
    let session = pane.last_claude_session_id();
    if session != st.session {
        st.session = session;
        st.tail = None;
        st.conv = Conversation::new();
        st.first = true;
    }
    match (&mut st.tail, &st.session) {
        (Some(tail), _) => {
            let read = tail.read_more();
            if read.reopened {
                st.conv = Conversation::new();
                st.first = true;
            }
            st.conv.apply_all(&read.lines);
            // Not there yet: the next poll looks for it again, in case it
            // appears under another project folder than the guessed one.
            if tail.waiting() {
                st.tail = None;
            }
        }
        (None, Some(_)) if st.looked.is_some_and(|t| t.elapsed().as_millis() < 1000) => {}
        (None, Some(id)) => {
            st.looked = Some(std::time::Instant::now());
            let home = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default());
            let cwd = pane.cached_cwd();
            if let Some(path) = crate::chat::transcript_path(&home, pane.claude_config_dir().as_deref(), cwd.as_deref(), id) {
                let (tail, lines) = Tail::open(&path);
                if !tail.waiting() {
                    log::info!("pane {}: chat reads {} ({} lines)", pane.id, path.display(), lines.len());
                }
                st.conv.apply_all(&lines);
                st.tail = Some(tail);
            }
        }
        (None, None) => {}
    }

    let working = pane.is_working();
    if !working {
        st.working_since = None;
    } else if st.working_since.is_none() {
        // The turn started when the user last wrote, if that is recent.
        st.working_since = Some(st.conv.last_user_ts().filter(|t| *t <= now && now - t < 6 * 3600).unwrap_or(now));
    }

    // The cards: this pane's questions, then its permission prompt.
    let mut questions = Vec::new();
    {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let session = pane.claude_session_id();
        crate::app::app_delegate(mtm).ivars().for_each_waiting_ask(|q| {
            if !cards::for_pane([q], pane.id, session.as_deref(), now).is_empty() {
                questions.push(q.clone());
            }
        });
    }
    let mut list: Vec<cards::Card> = questions
        .iter()
        .map(|q| cards::question_card(q, crate::handoff::detail(q).map(|d| d.card_text())))
        .collect();
    st.permission = pane.permission_prompt();
    if let Some(prompt) = &st.permission {
        list.push(cards::permission_card(prompt));
    }
    st.questions = questions;
    st.cards = list;

    let b = banner::banner(banner::BannerInput {
        has_session: pane.claude_session_id().is_some(),
        working,
        waiting: !st.cards.is_empty(),
        working_since: st.working_since,
        last_activity: st.conv.last_ts.or_else(|| pane.awaiting_since()),
    });

    // The message in flight.
    let verdict = if st.outbox.queued().is_some() { gate(pane) } else { input::Verdict::Wait("") };
    match st.outbox.poll(verdict, crate::surface::monotonic_ms()) {
        input::Step::Type(text) => type_into(pane, &text),
        input::Step::Enter => {
            // Only onto the text just pasted: the prompt still holds it and
            // nothing else came up meanwhile (a permission prompt, a menu).
            let visible = pane.terminal.read().dump_text(crate::terminal::DumpMode::Visible, true).text;
            let prompt = pane.has_permission_prompt() || crate::prompt_preview::parse_permission_prompt(&visible).is_some();
            if !prompt && input::composer(&visible) == input::Composer::Typed {
                pane.pty.write(b"\r");
                pane.clear_awaiting();
            } else {
                st.note = Some(NOT_ENTERED.to_string());
            }
        }
        input::Step::Stale => st.note = Some(NOT_ENTERED.to_string()),
        input::Step::Refused { reason, text } => {
            st.note = Some(reason.to_string());
            st.restore = Some(text);
        }
        input::Step::Queued(_) | input::Step::Nothing => {}
    }

    // Dictation: what was heard, a failure, the end of its thread.
    let mut voice_send = false;
    if let Some(session) = &st.voice {
        let ms = crate::surface::monotonic_ms();
        for note in session.take() {
            match note {
                crate::voice_io::Note::Event(e) => {
                    st.transcript.apply(&e, ms);
                }
                crate::voice_io::Note::Failed(line) => st.note = Some(line),
                crate::voice_io::Note::Closed => {
                    st.voice = None;
                    st.listening = false;
                    break;
                }
            }
        }
        if st.listening && crate::voice::auto_send() && st.transcript.auto_send_due(ms, crate::voice::AUTO_SEND_PAUSE_MS) {
            voice_send = true;
        }
    }

    if !st.ready {
        return None;
    }
    let mut update = page::Update { working, ..Default::default() };
    let mut changed = false;
    let banner = page::BannerWire { state: b.name(), text: banner::text(b, now) };
    if st.shown_banner.as_ref() != Some(&banner) {
        st.shown_banner = Some(banner.clone());
        update.banner = Some(banner);
        changed = true;
    }
    let key = (st.conv.revision, working, st.exchanges);
    if st.shown != Some(key) || st.first {
        st.shown = Some(key);
        let (entries, more) = page::entries(&st.conv, st.exchanges, working);
        update.entries = Some(page::wire(&entries, &mut st.sent_entries));
        update.more = more;
        changed = true;
    }
    let card_entries: Vec<page::Entry> =
        st.cards.iter().map(|c| page::Entry { id: c.id.clone(), html: page::card_html(c) }).collect();
    let card_key: Vec<(String, u64)> = card_entries.iter().map(|e| (e.id.clone(), e.hash())).collect();
    if st.shown_cards.as_ref() != Some(&card_key) {
        st.shown_cards = Some(card_key);
        update.cards = Some(page::wire(&card_entries, &mut st.sent_cards));
        changed = true;
    }
    let queued = st.outbox.queued().map(|t| page::Queued { text: t.to_string(), reason: st.queued_reason.to_string() });
    if st.shown_queued.as_ref() != Some(&queued) {
        st.shown_queued = Some(queued.clone());
        changed = true;
    }
    update.queued = queued;
    update.note = st.note.take();
    update.restore = st.restore.take();
    update.attach = (!st.attach.is_empty()).then(|| std::mem::take(&mut st.attach));
    let voice = page::Voice { on: st.listening, live: st.voice.is_some(), text: st.transcript.text(), send: voice_send };
    if voice_send {
        st.transcript.clear();
    }
    if st.shown_voice.as_ref() != Some(&voice) {
        st.shown_voice = Some(voice.clone());
        update.voice = Some(voice);
        changed = true;
    }
    update.first = std::mem::take(&mut st.first);
    update.focus = std::mem::take(&mut st.focus);
    changed |= update.note.is_some() || update.restore.is_some() || update.attach.is_some() || update.first || update.focus;
    changed.then_some(update)
}

#[derive(Default)]
pub struct ChatDelegateIvars {
    /// The page's messages since the last tick.
    inbox: RefCell<Vec<String>>,
    /// The page's process died: the next tick loads it again.
    crashed: Cell<bool>,
}

impl ChatDelegateIvars {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.inbox.borrow_mut())
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KovaChatWebDelegate"]
    #[ivars = ChatDelegateIvars]
    pub struct ChatWebDelegate;

    unsafe impl NSObjectProtocol for ChatWebDelegate {}

    unsafe impl WKNavigationDelegate for ChatWebDelegate {
        /// The page Kova loads, and nothing else. The page's script takes
        /// every link click (side panel, `link` message); should a click get
        /// through anyway, http(s) and mailto open in the default browser and
        /// every other navigation is cancelled.
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
                if clicked && (lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:"))
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
            log::warn!("chat: the page's process ended, loading it again");
            self.ivars().crashed.set(true);
        }
    }

    unsafe impl WKScriptMessageHandler for ChatWebDelegate {
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
                log::warn!("chat: ignored a message from {:?}", frame_url);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chat_page_takes_the_light_tokens_in_light_mode() {
        let mut c = crate::config::Config::default();
        crate::theme::apply_light(&mut c);
        tokens::set_background(c.colors.background);
        let light = page::shell(&theme(), "t", "n");
        let restyle = page::restyle_script(&theme());
        tokens::set_background(crate::config::Config::default().colors.background);
        let dark = page::shell(&theme(), "t", "n");
        assert!(light.contains("color-scheme: light"));
        assert!(light.contains("--bg:#fafafa;") && light.contains("--t1:#242424;"), "{light}");
        assert!(dark.contains("color-scheme: dark") && !dark.contains("--t1:#242424;"));
        // In place: every variable of the `:root` rule, light.
        assert!(restyle.contains("color-scheme:light;--bg:#fafafa;") && restyle.contains("--t1:#242424;") && restyle.contains("--ok:"), "{restyle}");
    }
}
