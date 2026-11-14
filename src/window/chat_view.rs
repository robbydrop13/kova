//! The Cockpit's chat on the window's side: a web view on the page's chat
//! column that shows the chief's conversation as a chat (`docs/chat-mode.md`,
//! the pure half in `crate::chat`).
//!
//! One web view per window, created on first use: it sits where the Cockpit
//! says (`cockpit_chat_target`), the chief's pane, its grid and PTY going on
//! underneath, and hides when the column is closed or while an overlay Kova
//! draws in Metal (the switcher, the search palette, help, a rename) needs
//! the window.
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

use crate::chat::{banner, cards, conversation::Conversation, input, page, tail::Tail, watch::Watch};
use crate::pane::Pane;
use crate::surface::PaneId;

use super::sidebar::tokens;
use super::KovaView;

/// How often the screen (cards, the prompt line) is read again. The
/// transcript and the hooks' turn file are read when the kqueue says they
/// changed, the spinner as soon as it flips.
const POLL_MS: u128 = 200;
/// The files are read this often anyway: a watch can miss a file not
/// written yet, or one replaced under it.
const FILES_FALLBACK_MS: u128 = 1_000;
/// A page message longer than this is not one the page sends.
const MESSAGE_MAX: usize = 64 * 1024;

/// The window's chat: its web view and what it shows.
#[derive(Default)]
pub struct ChatSlot {
    web: OnceCell<Retained<WKWebView>>,
    delegate: OnceCell<Retained<ChatWebDelegate>>,
    state: RefCell<Option<ChatState>>,
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
    /// The cards' HTML, in `cards`' order: an ask on the Cockpit's markup.
    card_html: Vec<String>,
    /// The attention entry standing for an ask, by ask id: what its icons
    /// and its instruction act on too.
    linked: HashMap<String, String>,
    /// `[attention] perso_dir`: an ask's scope tag.
    perso: Option<std::path::PathBuf>,
    permission: Option<crate::prompt_preview::ParsedPrompt>,
    /// The menu Claude Code waits on, read off the screen, and an answer
    /// to it under way: what was written last, for which question, when (ms).
    menu: Option<crate::prompt_preview::Menu>,
    menu_job: Option<(cards::MenuAnswer, cards::MenuStage, String, u64)>,
    shown: Option<(u64, bool, usize)>,
    shown_cards: Option<Vec<(String, u64)>>,
    shown_banner: Option<page::BannerWire>,
    shown_queued: Option<Vec<page::Queued>>,
    working_since: Option<u64>,
    /// The kqueue on the transcript and the turns folder, and when the files
    /// were last read.
    watch: Option<Watch>,
    files_read: Option<std::time::Instant>,
    /// What the session's hooks said last (`banner::parse_hook`).
    hook: Option<banner::HookTurn>,
    /// Claude Code's busy signal as last seen (`None` before the first
    /// look), and when it last came on and went off (epoch ms).
    spinner: Option<bool>,
    spinner_on_at: Option<u64>,
    spinner_off_at: Option<u64>,
    /// The turns folder was watched: it did not exist when the watch was made.
    watching_turns: bool,
    /// The page posted `ready`: updates reach it.
    ready: bool,
    first: bool,
    focus: bool,
    outbox: input::Outbox,
    /// The journal line of the message pasted and waiting for its Enter
    /// (`typed_keys::chat_entry`): written once the Enter is decided.
    sending: Option<Value>,
    /// Why the queue waits, as the gate said last.
    queue_reason: &'static str,
    note: Option<String>,
    restore: Option<String>,
    /// The images pasted into the box, by their files: only these go with a message.
    pasted: Vec<String>,
    attach: Vec<page::Attach>,
    /// Voice typing, and its pill as the page last got it.
    voice: crate::voice_io::Dictation,
    shown_voice: Option<crate::voice::Pill>,
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
            conv: Conversation::configured(),
            exchanges: page::EXCHANGES_STEP,
            sent_entries: HashMap::new(),
            sent_cards: HashMap::new(),
            questions: Vec::new(),
            cards: Vec::new(),
            card_html: Vec::new(),
            linked: HashMap::new(),
            perso: None,
            permission: None,
            menu: None,
            menu_job: None,
            shown: None,
            shown_cards: None,
            shown_banner: None,
            shown_queued: None,
            working_since: None,
            watch: None,
            files_read: None,
            hook: None,
            spinner: None,
            spinner_on_at: None,
            spinner_off_at: None,
            watching_turns: false,
            ready: false,
            first: true,
            focus: false,
            outbox: input::Outbox::default(),
            sending: None,
            queue_reason: "",
            note: None,
            restore: None,
            pasted: Vec::new(),
            attach: Vec::new(),
            voice: crate::voice_io::Dictation::default(),
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
}

fn css_hex(c: [f32; 3]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

/// The page's colours: the design system's, light or dark as the terminal is.
fn theme() -> page::Theme {
    theme_of(tokens::get())
}

/// The chat page's colours for a palette.
pub(crate) fn theme_of(p: &tokens::Palette) -> page::Theme {
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

impl KovaView {
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

    /// The chief's pane and where its chat goes on the Cockpit page, unless
    /// an overlay Kova draws in Metal needs the window.
    fn chat_target(&self) -> Option<(PaneId, (f64, f64, f64, f64))> {
        if self.metal_overlay_open() {
            return None;
        }
        self.cockpit_chat_target()
    }

    /// Once per frame: place the web view on the Cockpit's chat column (or
    /// hide it), give it the keyboard when it shows, serve the page's
    /// messages and push what changed.
    pub(super) fn tick_chat(&self) {
        let Some(window) = self.window() else { return };
        let slot = &self.ivars().chat;
        let target = self.chat_target();
        let Some((pane_id, frame)) = target else {
            if let Some(web) = slot.web.get()
                && !web.isHidden()
            {
                if window.firstResponder().is_some_and(|r| self.chat_holds(&r)) {
                    window.makeFirstResponder(Some(self));
                }
                web.setHidden(true);
            }
            // Out of sight (the column closed, or an overlay over it), the
            // state goes: the page comes back fresh and takes the keyboard,
            // and dropping it cancels what the mic hears.
            if slot.state.borrow().is_some() {
                slot.state.replace(None);
                if let Some(web) = slot.web.get() {
                    unsafe { web.loadHTMLString_baseURL(&NSString::from_str(""), None) };
                }
            }
            return;
        };
        let mtm = MainThreadMarker::from(self);
        let web = self.chat_web_view(mtm);
        if slot.delegate.get().is_some_and(|d| d.ivars().crashed.take()) {
            slot.state.replace(None);
        }
        let generation = crate::theme::generation();
        let fresh = slot.state.borrow().as_ref().is_none_or(|st| st.pane_id != pane_id);
        if fresh {
            slot.theme.set(generation);
            let nonce = crate::ask::new_question_id().replace('-', "");
            slot.state.replace(Some(ChatState::new(pane_id, nonce.clone())));
            let title = self.with_pane(pane_id, |p| p.display_title("Chat")).unwrap_or_default();
            let html = page::shell(&theme(), &title, &nonce);
            unsafe { web.loadHTMLString_baseURL(&NSString::from_str(&html), None) };
        }
        // The theme switched: the colours change in place, the page keeps
        // its input, its scroll and what it shows.
        if slot.theme.get() != generation {
            slot.theme.set(generation);
            unsafe { web.evaluateJavaScript_completionHandler(&NSString::from_str(&page::restyle_script(&theme())), None) };
        }
        if slot.frame.get() != Some(frame) {
            let (x, y, w, h) = frame;
            web.setFrame(CGRect { origin: CGPoint { x, y }, size: CGSize { width: w, height: h } });
            slot.frame.set(Some(frame));
        }
        if web.isHidden() {
            web.setHidden(false);
        }

        // The box gets the keyboard when the chat shows. The chief's pane is
        // never focused for it: nothing moves the focus or switches tabs.
        let on_terminal = window
            .firstResponder()
            .is_some_and(|r| std::ptr::eq(Retained::as_ptr(&r).cast::<()>(), (self as *const Self).cast::<()>()));
        if on_terminal || fresh {
            window.makeFirstResponder(Some(&web));
            if let Some(st) = slot.state.borrow_mut().as_mut() {
                st.focus = true;
            }
        }

        // The page's messages, then the poll.
        let messages = slot.delegate.get().map(|d| d.ivars().take()).unwrap_or_default();
        let mut update = None;
        let mut open = Vec::new();
        {
            let tabs = self.ivars().tabs.borrow();
            let Some(pane) = tabs.iter().find_map(|t| t.pane(pane_id)) else { return };
            let mut state = slot.state.borrow_mut();
            let Some(st) = state.as_mut() else { return };
            let mut force = false;
            for raw in messages {
                match self.chat_message(st, pane, &raw) {
                    Reply::Push => force = true,
                    Reply::Open(target) => {
                        open.push(target);
                        force = true;
                    }
                    Reply::Nothing => {}
                }
            }
            // The session moved (a file written, the spinner flipped) or a
            // message is on its way: now, not at the next poll.
            let files = st.watch.as_ref().is_some_and(Watch::fired);
            let moved = files || Some(pane.is_working()) != st.spinner || st.outbox.enter_pending();
            let due = st.last_poll.is_none_or(|t| t.elapsed().as_millis() >= POLL_MS);
            if due || force || moved {
                st.last_poll = Some(std::time::Instant::now());
                st.perso = self.ivars().config.get().and_then(|c| c.attention.perso_root());
                update = chat_poll(st, pane, files);
            }
        }
        if let Some(update) = update {
            unsafe { web.evaluateJavaScript_completionHandler(&NSString::from_str(&page::update_script(&update)), None) };
        }
        // Outside the borrow of the tabs above (crash of 06/11: RefCell already borrowed in side_panel_pins).
        for t in open {
            self.open_in_side_panel(t);
        }
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
                read_enough(st);
                Reply::Push
            }
            "cancel" => {
                // Taken back: its text returns to the box, to rework or drop.
                match msg.get("id").and_then(Value::as_u64).and_then(|id| st.outbox.cancel(id)) {
                    Some(text) => st.restore = Some(text),
                    None => st.note = Some("Déjà envoyé".to_string()),
                }
                Reply::Push
            }
            "edit" => {
                let id = msg.get("id").and_then(Value::as_u64).unwrap_or_default();
                match input::sanitize(&s("text")) {
                    Ok(text) => {
                        if !st.outbox.edit(id, text) {
                            st.note = Some("Déjà envoyé".to_string());
                        }
                    }
                    Err(reason) => st.note = Some(reason.to_string()),
                }
                st.shown_queued = None;
                Reply::Push
            }
            "voice-press" => {
                st.voice.press(crate::surface::monotonic_ms(), None);
                Reply::Push
            }
            "voice-release" => {
                st.voice.release(crate::surface::monotonic_ms());
                Reply::Push
            }
            "voice-finish" => {
                st.voice.finish();
                Reply::Push
            }
            "voice-cancel" => {
                st.voice.cancel();
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
                        Some(target) => return Reply::Open(target),
                        None if crate::links::is_web(&href) => st.note = Some("Lien non pris en charge".to_string()),
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
                let text = input::with_images(&s("text"), &images);
                // A menu that takes text waits: the box answers it, at once.
                if let Some(menu) = &st.menu
                    && let Some(answer) = cards::box_answer(menu, &text)
                {
                    let id = cards::menu_id(menu);
                    if let Err(e) = start_menu_answer(st, pane, &id, answer) {
                        st.note = Some(e);
                        st.restore = Some(text);
                    }
                    return Reply::Push;
                }
                match input::sanitize(&text) {
                    Err(reason) => {
                        st.note = Some(reason.to_string());
                        st.restore = Some(text);
                    }
                    // Through the queue: the poll this same tick decides on
                    // the state read now, not the last poll's.
                    Ok(text) => match st.outbox.submit(text, input::Verdict::Wait(""), crate::surface::monotonic_ms()) {
                        input::Step::Type(text) => st.sending = Some(type_into(pane, &text)),
                        input::Step::Queued(reason) => {
                            if !reason.is_empty() {
                                st.queue_reason = reason;
                            }
                        }
                        input::Step::Enter | input::Step::Stale | input::Step::Nothing => {}
                    },
                }
                Reply::Push
            }
            "answer" if s("id").starts_with("menu:") => {
                let (id, action, text) = (s("id"), s("action"), s("text"));
                let answer = match st.menu.as_ref().filter(|m| cards::menu_id(m) == id) {
                    Some(menu) => cards::menu_answer(menu, &action, &text).map_err(str::to_string),
                    None => Err("Ce menu n'est plus à l'écran".to_string()),
                };
                if let Err(e) = answer.and_then(|a| start_menu_answer(st, pane, &id, a)) {
                    st.note = Some(e);
                    st.sent_cards.remove(&id);
                    st.shown_cards = None;
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
                            .queue_ask_action(super::ask_actions::AskAction::Answer { id: id.clone(), action, text });
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
            "ask" => {
                self.chat_ask_act(st, &s("id"), &s("act"), &s("text"));
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

    /// An ask card's icon or instruction, as the Cockpit's: snooze puts the
    /// question off until the date picked, remove (`dismissed`) and done take
    /// it off the Cockpit, its asker told it was withdrawn. The attention entry
    /// standing for it gets the same verb (`attn snooze`, `attn close --reason
    /// dismissed|done`), and the instruction (`attn act ID instruct`), which
    /// only such an entry takes. Queued or spawned: nothing here touches the
    /// window (this runs under the tabs' borrow).
    fn chat_ask_act(&self, st: &mut ChatState, id: &str, act: &str, text: &str) {
        use super::ask_actions::AskAction;
        if !st.questions.iter().any(|q| q.id == id) {
            st.note = Some("Cette question n'attend plus".to_string());
            return;
        }
        let entry = st.linked.get(id).cloned();
        let mtm = MainThreadMarker::from(self);
        let queue = |action| crate::app::app_delegate(mtm).ivars().queue_ask_action(action);
        let attn = |verb, text: Option<&str>| {
            if let Some(e) = &entry
                && let Err(why) = crate::attention::perform_with(e, verb, text, None)
            {
                log::warn!("chat: {why}");
            }
        };
        let mut refused = None;
        match act {
            "snooze" => match crate::attention::epoch_of(text) {
                Some(until) if until > crate::ask::now_epoch() => {
                    queue(AskAction::Snooze { id: id.to_string(), until });
                    attn(crate::attention::Verb::Snooze, Some(text));
                }
                _ => refused = Some("Date de rappel invalide"),
            },
            "dismiss" | "done" => {
                let (reason, verb) =
                    if act == "done" { ("done", crate::attention::Verb::Resolve) } else { ("dismissed", crate::attention::Verb::Dismiss) };
                queue(AskAction::Close { id: id.to_string(), reason });
                attn(verb, None);
            }
            "instruct" if !text.trim().is_empty() => match &entry {
                Some(e) => {
                    if let Err(why) = crate::attention::perform_with(e, crate::attention::Verb::Instruct, Some(text), None) {
                        log::warn!("chat: {why}");
                        crate::attention::instruction_refused(e, text, "Kova n'a pas pu la transmettre");
                    }
                }
                None => refused = Some("Pas de consigne sur cette question"),
            },
            _ => {}
        }
        // A refusal says so and the card the page hid comes back; a click
        // taken leaves it hidden until the next read drops it.
        if let Some(why) = refused {
            st.note = Some(why.to_string());
            st.sent_cards.remove(id);
            st.shown_cards = None;
        }
    }

    /// `toggle-voice`: the shortcut went down in the chat shown (held,
    /// push-to-talk; tapped, hands-free until the next press).
    pub(super) fn toggle_voice(&self) {
        let toggled = match self.ivars().chat.state.borrow_mut().as_mut() {
            Some(st) if st.ready => {
                st.voice.shortcut();
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
    /// Open a link in the side panel, once the tabs are no longer borrowed:
    /// opening it lays the window out again, which borrows the tabs mutably.
    Open(super::side_panel::PanelTarget),
}

/// Paste `text` into the pane, bracketed when the app asked for it. Returns
/// its journal line, to write once its Enter is decided (`journal_send`).
fn type_into(pane: &Pane, text: &str) -> Value {
    let bracketed = pane.terminal.read().bracketed_paste;
    log::info!("pane {}: chat typed {} bytes", pane.id, text.len());
    let typed = input::typed(text, bracketed);
    let target = crate::ipc::host::typing_target_of(pane);
    let line = crate::typed_keys::chat_entry(&crate::typed_keys::now_utc(), pane.id, &target, &typed, crate::typed_keys::policy().mode);
    pane.paste(&typed);
    line
}

/// The message typed last goes to the send-keys journal with how its Enter
/// ended: what the user typed himself in the chat is on record like what a
/// tool typed through the socket, without its text.
fn journal_send(st: &mut ChatState, enter: crate::typed_keys::Enter) {
    let Some(line) = st.sending.take() else { return };
    if let Some(journal) = crate::typed_keys::policy().journal {
        crate::typed_keys::append(&journal, &crate::typed_keys::chat_entered(line, enter));
    }
}

/// The pane's screen as text, and whether its input line holds only greyed
/// text (`TerminalState::prompt`): what `input::composer` reads.
fn screen(pane: &Pane) -> (String, bool) {
    let term = pane.terminal.read();
    (term.dump_text(crate::terminal::DumpMode::Visible, true).text, term.prompt().is_some_and(|p| p.ghost))
}

/// Whether the pane's prompt takes a message now (`chat::input::gate`).
/// `ended`: Stop ran since the last prompt (`banner::turn_ended`), so a busy
/// pane is only waiting for its background agents.
fn gate(pane: &Pane, status: banner::Banner, ended: bool) -> input::Verdict {
    let (visible, ghost) = screen(pane);
    // A permission prompt that came up since the last poll waits too.
    let prompt = pane.has_permission_prompt()
        || crate::prompt_preview::parse_permission_prompt(&visible).is_some()
        || crate::prompt_preview::parse_menu(&visible).is_some();
    input::gate(input::GateInput {
        // The session id lags Claude's exit; a fresh probe of the foreground
        // process does not, so a shell's own `❯` never reads as Claude's.
        has_session: pane.claude_session_id().is_some() && pane.pty.foreground_process().is_some(),
        status: if prompt {
            banner::Banner::Waiting
        } else if pane.is_working() && !ended {
            banner::Banner::Working { since: None }
        } else {
            status
        },
        composer: input::composer(&visible, ghost),
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
const NOT_ENTERED: &str = "Pas envoyé : le texte attend dans l'invite du terminal, Entrée là-bas";

fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The window read from the transcript's end holds bytes, not exchanges: a
/// pasted image or long tool results fill it with a few minutes of
/// conversation. Read from further back until the chat has the exchanges it
/// shows, or the file (or `tail::MAX_WINDOW_BYTES`) is exhausted.
fn read_enough(st: &mut ChatState) {
    let Some(tail) = st.tail.as_mut() else { return };
    while st.conv.user_count() < st.exchanges && tail.can_widen() {
        let lines = tail.widen();
        let mut conv = Conversation::configured();
        conv.apply_all(&lines);
        log::info!("chat reads further back: {} lines, {} exchanges", lines.len(), conv.user_count());
        st.conv = conv;
        st.shown = None;
    }
}

/// The transcript: what was appended since the last read, or the file
/// looked for (and watched) once it exists.
fn read_transcript(st: &mut ChatState, pane: &Pane, home: &std::path::Path) {
    // A window just read from the end may hold too few exchanges.
    let mut enough = true;
    match (&mut st.tail, &st.session) {
        (Some(tail), _) => {
            let read = tail.read_more();
            let reopened = read.reopened;
            if reopened {
                st.conv = Conversation::configured();
                st.first = true;
                st.watch = None;
            }
            st.conv.apply_all(&read.lines);
            enough = !reopened;
            // Not there yet: the next read looks for it again, in case it
            // appears under another project folder than the guessed one.
            if tail.waiting() {
                st.tail = None;
            }
        }
        (None, Some(_)) if st.looked.is_some_and(|t| t.elapsed().as_millis() < 1000) => {}
        (None, Some(id)) => {
            st.looked = Some(std::time::Instant::now());
            let cwd = pane.cached_cwd();
            if let Some(path) = crate::chat::transcript_path(home, pane.claude_config_dir().as_deref(), cwd.as_deref(), id) {
                let (tail, lines) = Tail::open(&path);
                if !tail.waiting() {
                    log::info!("pane {}: chat reads {} ({} lines)", pane.id, path.display(), lines.len());
                }
                st.conv.apply_all(&lines);
                st.tail = Some(tail);
                st.watch = None;
                enough = false;
            }
        }
        (None, None) => {}
    }
    if !enough {
        read_enough(st);
    }
    // Watch what is read: the transcript (or its folder until it exists)
    // and the hooks' turns folder, where a turn file is renamed into place.
    let turns = home.join(".claude/state/turns");
    if st.watch.is_some() && !st.watching_turns && turns.is_dir() {
        st.watch = None;
    }
    if st.watch.is_none()
        && let (Some(tail), Some(_)) = (&st.tail, &st.session)
    {
        st.watching_turns = turns.is_dir();
        let file = tail.path();
        let target = if tail.waiting() { file.parent().unwrap_or(file) } else { file };
        st.watch = Watch::new(&[target, &turns]);
    }
}

/// Between two writes of a menu answer: Claude Code's menu reads a digit,
/// text and Enter written together as one key (measured on 2.1.319).
const MENU_STEP_MS: u64 = 250;

/// Answer the menu `id` names: the screen is read again, and the option's
/// digit goes only if it is still that menu. The rest follows in `menu_step`.
fn start_menu_answer(st: &mut ChatState, pane: &Pane, id: &str, answer: cards::MenuAnswer) -> Result<(), String> {
    if st.menu_job.is_some() {
        return Err("Une réponse est déjà en cours".to_string());
    }
    let visible = pane.terminal.read().dump_text(crate::terminal::DumpMode::Visible, true).text;
    let Some(menu) = crate::prompt_preview::parse_menu(&visible).filter(|m| cards::menu_id(m) == id) else {
        return Err("Le menu a changé : regarde le terminal".to_string());
    };
    log::info!("pane {}: menu answered with option {}", pane.id, answer.number);
    pane.pty.write(answer.number.to_string().as_bytes());
    st.menu_job = Some((answer, cards::MenuStage::Digit, menu.question, now_epoch_ms()));
    Ok(())
}

/// The next write of a menu answer under way, once `MENU_STEP_MS` passed:
/// the text, Enter, or nothing (the menu took the digit). A text that goes
/// after it joins the queue, which sends it once the turn allows.
fn menu_step(st: &mut ChatState, pane: &Pane, now_ms: u64) {
    let Some((answer, stage, question, at)) = st.menu_job.clone() else { return };
    if now_ms < at + MENU_STEP_MS {
        return;
    }
    match cards::menu_next(&answer, stage, &question, st.menu.as_ref()) {
        Ok(Some(bytes)) if bytes != "\r" => {
            pane.pty.write(bytes.as_bytes());
            st.menu_job = Some((answer, cards::MenuStage::Text, question, now_ms));
            return;
        }
        Ok(Some(enter)) => pane.pty.write(enter.as_bytes()),
        Ok(None) => {}
        Err(why) => {
            st.note = Some(why.to_string());
            // The text did not go: back in the box, and the card again.
            st.restore = answer.text.or(answer.after);
            st.menu_job = None;
            st.shown_cards = None;
            st.sent_cards.clear();
            return;
        }
    }
    st.menu_job = None;
    if let Some(text) = answer.after {
        match input::sanitize(&text) {
            Ok(text) => {
                st.outbox.submit(text, input::Verdict::Wait(""), crate::surface::monotonic_ms());
            }
            Err(reason) => st.note = Some(reason.to_string()),
        }
    }
}

/// Read the transcript, the cards and the gate again; the update to push,
/// if the page is ready and something changed. `files`: the kqueue said the
/// transcript or the turn file changed.
fn chat_poll(st: &mut ChatState, pane: &Pane, files: bool) -> Option<page::Update> {
    let now = crate::ask::now_epoch();
    let now_ms = now_epoch_ms();
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default());
    // The conversation: a new id (a `/clear`, a resume) is another transcript.
    let session = pane.last_claude_session_id();
    if session != st.session {
        st.session = session;
        st.tail = None;
        st.watch = None;
        st.hook = None;
        st.conv = Conversation::configured();
        st.first = true;
    }
    let read_files = files || st.files_read.is_none_or(|t| t.elapsed().as_millis() >= FILES_FALLBACK_MS);
    if read_files {
        st.files_read = Some(std::time::Instant::now());
        read_transcript(st, pane, &home);
        st.hook = st
            .session
            .as_deref()
            .and_then(|id| banner::hook_path(&home, id))
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| banner::parse_hook(&t));
    }

    let spinner = pane.is_working();
    match (st.spinner, spinner) {
        (Some(true), false) => st.spinner_off_at = Some(now_ms),
        (Some(false), true) => st.spinner_on_at = Some(now_ms),
        _ => {}
    }
    st.spinner = Some(spinner);

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
    let mut list: Vec<cards::Card> = Vec::new();
    let mut html = Vec::new();
    st.linked.clear();
    // An ask is the Cockpit's card: the attention entry standing for it when
    // there is one, opened (its detail, its instruction field).
    let entries = crate::attention::snapshot();
    let (outcomes, instructions) = (crate::attention::outcomes(), crate::attention::instructions());
    let mut waiting_ids = Vec::new();
    {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        crate::app::app_delegate(mtm).ivars().for_each_waiting_ask(|q| waiting_ids.push(q.id.clone()));
    }
    let waiting: Vec<&str> = waiting_ids.iter().map(String::as_str).collect();
    for q in &questions {
        let handoff = crate::handoff::detail(q).map(|d| d.card_text());
        let linked = entries.as_deref().and_then(|e| crate::cockpit::entry_for_ask(e, &q.id, &waiting)).and_then(|e| {
            let mut item = crate::cockpit::from_attention(e, outcomes.get(&e.id), false)?;
            item.instruction = instructions.get(&e.id).cloned();
            item.detail = Some(crate::cockpit::entry_detail(e, crate::attention::explaining(&e.id)));
            st.linked.insert(q.id.clone(), e.id.clone());
            Some(item)
        });
        let scope = q.from.cwd.as_deref().map_or(crate::attention::Scope::Pro, |cwd| crate::cockpit::scope_of_cwd(cwd, st.perso.as_deref()));
        html.push(cards::ask_card_html(q, handoff.as_deref(), linked.as_ref(), crate::attention::can_act(), scope, now));
        list.push(cards::question_card(q, handoff));
    }
    st.permission = pane.permission_prompt();
    // A menu on screen (permission, AskUserQuestion, plan approval), read
    // only when something says the session waits.
    let waits = st.conv.question_pending()
        || st.hook.is_some_and(|h| h.turn == banner::Turn::Waiting)
        || st.permission.is_some()
        || pane.awaiting_since().is_some()
        || st.menu_job.is_some();
    st.menu = (!spinner && waits)
        .then(|| crate::prompt_preview::parse_menu(&pane.terminal.read().dump_text(crate::terminal::DumpMode::Visible, true).text))
        .flatten();
    menu_step(st, pane, now_ms);
    if let Some(menu) = &st.menu {
        html.push(cards::menu_card_html(menu, st.conv.pending_plan(), now));
        list.push(cards::menu_card(menu));
    } else if let Some(prompt) = &st.permission {
        let card = cards::permission_card(prompt);
        html.push(page::card_html(&card));
        list.push(card);
    }
    st.card_html = html;
    st.questions = questions;
    st.cards = list;

    // The turn's state. The screen is read only when a question may be up.
    let question = st.conv.question_pending();
    let asked = question || st.hook.is_some_and(|h| h.turn == banner::Turn::Waiting);
    let prompt_line = !(asked && !spinner) || {
        let (visible, ghost) = screen(pane);
        input::composer(&visible, ghost) != input::Composer::Unreadable
    };
    let status_input = banner::BannerInput {
        has_session: pane.claude_session_id().is_some(),
        spinner,
        spinner_on_at: st.spinner_on_at,
        spinner_off_at: st.spinner_off_at,
        cards: !st.cards.is_empty(),
        question,
        prompt_line,
        hook: st.hook,
        now_ms,
        working_since: st.working_since,
        last_activity: st.conv.last_ts.or_else(|| pane.awaiting_since()),
    };
    let working = matches!(banner::banner(status_input), banner::Banner::Working { .. });
    if !working {
        st.working_since = None;
    } else if st.working_since.is_none() {
        // The turn started when the user last wrote, if that is recent.
        st.working_since = Some(st.conv.last_user_ts().filter(|t| *t <= now && now - t < 6 * 3600).unwrap_or(now));
    }
    let b = banner::banner(banner::BannerInput { working_since: st.working_since, ..status_input });
    // Waiting on something no card shows: say so rather than nothing.
    if b == banner::Banner::Waiting && st.cards.is_empty() {
        st.cards.push(cards::Card {
            id: "wait:unread".to_string(),
            kind: cards::CardKind::Menu,
            from: String::new(),
            message: String::new(),
            detail: None,
            input: false,
            buttons: Vec::new(),
            hint: None,
        });
        st.card_html.push(cards::unread_wait_html(now));
    }

    // A text left in the terminal's prompt went (the turn started) or was
    // cleared there: the queue goes on. Claude Code's greyed suggestion in
    // the emptied prompt is no text left.
    if st.outbox.is_stuck()
        && (working || {
            let (visible, ghost) = screen(pane);
            input::composer(&visible, ghost) == input::Composer::Empty
        })
    {
        st.outbox.unstick();
    }
    if st.outbox.is_stuck() {
        st.queue_reason = "part quand le texte laissé dans le terminal est envoyé ou effacé";
    }
    // The queue: its first message once the turn is over.
    // Busy since the last poll is a new turn, not a background wait.
    let ended = banner::turn_ended(st.hook, st.spinner_on_at) && st.spinner == Some(true);
    let verdict = if st.outbox.queue().is_empty() { input::Verdict::Wait("") } else { gate(pane, b, ended) };
    if let input::Verdict::Wait(reason) = verdict
        && !reason.is_empty()
    {
        st.queue_reason = reason;
    }
    match st.outbox.poll(verdict, crate::surface::monotonic_ms()) {
        input::Step::Type(text) => st.sending = Some(type_into(pane, &text)),
        input::Step::Enter => {
            // Only onto the text just pasted: the prompt still holds it and
            // nothing else came up meanwhile (a permission prompt, a menu).
            let (visible, ghost) = screen(pane);
            let prompt = pane.has_permission_prompt()
                || crate::prompt_preview::parse_permission_prompt(&visible).is_some()
                || crate::prompt_preview::parse_menu(&visible).is_some();
            let composer = input::composer(&visible, ghost);
            let now = crate::surface::monotonic_ms();
            if !prompt && composer == input::Composer::Typed {
                pane.pty.write(b"\r");
                pane.clear_awaiting();
                log::info!("pane {}: chat Enter written", pane.id);
                journal_send(st, crate::typed_keys::Enter::Written);
            } else if !prompt && composer == input::Composer::Empty && st.outbox.not_shown(now) {
                // The paste is not drawn yet (a greyed suggestion is not it):
                // the Enter waits for it, `ENTER_STALE_MS` at most.
            } else {
                st.outbox.stuck();
                st.note = Some(NOT_ENTERED.to_string());
                log::warn!("pane {}: chat Enter held (dialog: {prompt}, composer: {composer:?})", pane.id);
                journal_send(st, crate::typed_keys::Enter::Held);
            }
        }
        input::Step::Stale => {
            st.note = Some(NOT_ENTERED.to_string());
            log::warn!("pane {}: chat Enter dropped, too late", pane.id);
            journal_send(st, crate::typed_keys::Enter::Dropped);
        }
        input::Step::Queued(_) | input::Step::Nothing => {}
    }

    // Voice typing: its pill, and the text of a dictation that just ended.
    let polled = st.voice.poll();

    if !st.ready {
        return None;
    }
    let mut update = page::Update { working, ..Default::default() };
    let queued: Vec<page::Queued> = st.outbox.queue().iter().map(|p| page::Queued { id: p.id, text: p.text.clone() }).collect();
    let mut changed = false;
    // The queue's reason goes with the state: `Travaille · 2 en attente, part à la fin du tour`.
    let mut text = banner::text(b, now);
    if !queued.is_empty() {
        text = format!("{text} · {} en attente, {}", queued.len(), st.queue_reason);
    }
    let answers = st.menu.as_ref().is_some_and(|m| m.free_text().is_some());
    let banner = page::BannerWire { state: b.name(), text, answers };
    if st.shown_banner.as_ref() != Some(&banner) {
        st.shown_banner = Some(banner.clone());
        update.banner = Some(banner);
        changed = true;
    }
    let key = (st.conv.revision, working, st.exchanges);
    if st.shown != Some(key) || st.first {
        st.shown = Some(key);
        crate::links::set_base(pane.cached_cwd().as_deref().map(std::path::Path::new));
        let (entries, more) = page::entries(&st.conv, st.exchanges, working);
        // Only the chat has a pane's folder: every other page reads none.
        crate::links::set_base(None);
        update.entries = Some(page::wire(&entries, &mut st.sent_entries));
        // Earlier exchanges may also be in the file, before what was read.
        update.more = more || st.tail.as_ref().is_some_and(|t| t.can_widen());
        changed = true;
    }
    let card_entries: Vec<page::Entry> =
        st.cards.iter().zip(&st.card_html).map(|(c, h)| page::Entry { id: c.id.clone(), html: h.clone() }).collect();
    let card_key: Vec<(String, u64)> = card_entries.iter().map(|e| (e.id.clone(), e.hash())).collect();
    if st.shown_cards.as_ref() != Some(&card_key) {
        st.shown_cards = Some(card_key);
        update.cards = Some(page::wire(&card_entries, &mut st.sent_cards));
        changed = true;
    }
    if st.shown_queued.as_ref() != Some(&queued) {
        st.shown_queued = Some(queued.clone());
        update.queued = Some(queued);
        changed = true;
    }
    update.note = st.note.take();
    update.restore = st.restore.take();
    update.attach = (!st.attach.is_empty()).then(|| std::mem::take(&mut st.attach));
    if st.shown_voice.as_ref() != Some(&polled.pill) {
        st.shown_voice = Some(polled.pill.clone());
        update.voice = Some(polled.pill);
        changed = true;
    }
    update.dictated = polled.text.map(|text| page::Dictated { text, send: crate::voice::auto_send() });
    update.first = std::mem::take(&mut st.first);
    update.focus = std::mem::take(&mut st.focus);
    changed |= update.note.is_some() || update.restore.is_some() || update.attach.is_some() || update.dictated.is_some() || update.first || update.focus;
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
