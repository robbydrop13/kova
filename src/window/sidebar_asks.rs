//! The Asks view of the sidebar: the open `kova ask` questions and the
//! permission prompts waiting in panes, as cards. A question card carries the
//! two answers the user actually needs (OK and KO), Brief, Snooze and the pane
//! it came from; a permission card carries Approve, Reject, Always when the
//! prompt offers it, the command in a code block and the pane to open. Pure:
//! `layout` turns `AsksState` into rects, `hit` turns a point into an
//! `AsksHit`, `click` runs the state machine and returns the action to queue
//! on the delegate. `sidebar_ui` builds the cards from `ask::Board` and the
//! panes' previews, `sidebar_view` draws them and dispatches the clicks.
//!
//! The card design: `bg1` ground, 1 pt `b3` border, radius 8,
//! 10/12 padding, 8 pt apart. A highlighted card takes the selected rail's
//! colour as its border. See `docs/sidebar-spec.md`.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use objc2::runtime::AnyObject;
use objc2_app_kit::{NSColor, NSFont, NSLineBreakMode, NSMutableParagraphStyle, NSStringDrawing};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSDictionary, NSString};

use super::sidebar::{format_age, tokens};
use super::sidebar_view::{
    AppKitMetrics, Align, EDGE_TOLERANCE, LIST_BOTTOM, LIST_TOP, ListHit, PAD_H, Rect, Shared, Style, TextMetrics,
    draw_text, draw_wrapped, fill_dot, fill_round, stroke_icon, stroke_round,
};
use crate::pane::PaneId;
use crate::prompt_preview::{ParsedPrompt, PermissionChoice};

// ---------------------------------------------------------------
// Metrics (points)
// ---------------------------------------------------------------

/// The card: radius, padding, gap under it.
const CARD_RADIUS: f64 = 8.0;
const CARD_PAD_V: f64 = 10.0;
const CARD_PAD_H: f64 = 12.0;
const CARD_GAP: f64 = 8.0;
/// The header line: a warn dot, `from`, the tab, the age.
const HEADER_H: f64 = 18.0;
const HEADER_GAP: f64 = 6.0;
const HEADER_TEXT_X: f64 = 16.0;
const HEADER_TAB_GAP: f64 = 6.0;
const STATE_DOT_D: f64 = 8.0;
/// The message: 12 pt, `MSG_LINE_H` per line, six lines then an ellipsis.
const MSG_LINE_H: f64 = 15.0;
const MSG_MAX_LINES: usize = 6;
/// The action row: OK/KO/Brief, 32 x 28, radius 6, 6 apart.
const AB_W: f64 = 32.0;
const AB_H: f64 = 28.0;
const AB_RADIUS: f64 = 6.0;
const AB_GAP: f64 = 6.0;
const AB_ICON: f64 = 16.0;
/// The ghost buttons at the right of the action row: 26 wide, 2 apart.
const GHOST_W: f64 = 26.0;
const GHOST_GAP: f64 = 2.0;
const GHOST_ICON: f64 = 14.0;
/// Between the message and the action row, and under the actions.
const ACT_GAP: f64 = 8.0;
/// The snooze row: three small buttons under the action row.
const SNOOZE_ROW_GAP: f64 = 6.0;
const SNOOZE_BTN_H: f64 = 22.0;
const SNOOZE_BTN_GAP: f64 = 6.0;
const SNOOZE_BTN_PAD: f64 = 6.0;
const SNOOZE_RADIUS: f64 = 6.0;
/// The line under the actions: what OK and KO send. 11 pt.
const NOTE_GAP: f64 = 6.0;
const NOTE_H: f64 = 14.0;
const NOTE_ICON: f64 = 10.0;
const NOTE_ICON_GAP: f64 = 4.0;
/// From the end of the first option's label to the second icon: the pair
/// reads as one line, not two columns.
const NOTE_OPTION_GAP: f64 = 6.0;
/// A sending or sent card's status line, where the actions used to be.
const STATUS_H: f64 = 28.0;
const STATUS_ICON: f64 = 13.0;
/// The folded `Snoozed N` header, and the gap under it when open.
const SNOOZE_HEADER_H: f64 = 28.0;
const SNOOZE_CONTENT_GAP: f64 = 6.0;
const SNOOZE_CHEVRON: f64 = 12.0;
const SNOOZE_TEXT_X: f64 = 20.0;
/// A snoozed card's footer: `back at 20:00` and `Show now`.
const FOOT_H: f64 = 24.0;
const SHOW_NOW_PAD: f64 = 6.0;
/// The empty state.
const EMPTY_H: f64 = 96.0;
const EMPTY_ICON: f64 = 30.0;
/// A card is urgent from two hours on: its age turns to the warn colour.
const URGENT_AFTER_SECS: u64 = 2 * 3600;
/// How long `Sent to ...` stays after the board drops the question.
const SENT_LINGER_MS: u64 = 1500;
/// How long a click may sit in `Sending` before it gives up and lets the
/// card be clicked again (the queue answered nothing).
const SENDING_TIMEOUT_MS: u64 = 3000;
/// A highlight (a need row walked to its card) lasts two seconds.
pub const HIGHLIGHT_MS: u64 = 2000;
// A permission card: the `Permission` tag in the header, the command in a
// monospace block, and the crit line a refused click leaves for 5 s.
const TAG_H: f64 = 16.0;
const TAG_PAD_H: f64 = 5.0;
const TAG_RADIUS: f64 = 4.0;
const TAG_TEXT: &str = "Permission";
const DECISION_TAG_TEXT: &str = "Decision";
/// A decision card's Open: a text button on the action row, accent ink.
const OPEN_DOC_TEXT: &str = "Open";
/// Between the question and the code block, and the block and the actions.
const CODE_GAP: f64 = 8.0;
const CODE_RADIUS: f64 = 4.0;
const CODE_PAD: f64 = 6.0;
const CODE_LINE_H: f64 = 14.0;
const CODE_MAX_LINES: usize = 4;
/// Menlo 11 at 0.6023 em: the block's own wrap, so the layout and the drawn
/// lines agree without a `TextMetrics` width for a monospace face.
const CODE_ADVANCE: f64 = 6.63;
const MONO_SIZE: f64 = 11.0;
/// The `Always` text button: a small chip on the action row.
const ALWAYS_PAD: f64 = 8.0;
/// How long a permission card says `Prompt changed, open the pane`.
pub const FLASH_MS: u64 = 5000;
pub const FLASH_TEXT: &str = "Prompt changed, open the pane";

// The Brief overlay, in points: a card centred at the top of the list, over a
// 40 % black scrim, holding the question, its options as chips, the
// instruction field and the two buttons.
const BRIEF_TOP: f64 = 12.0;
const BRIEF_SIDE: f64 = 12.0;
const BRIEF_RADIUS: f64 = 8.0;
const BRIEF_PAD: f64 = 12.0;
const BRIEF_GAP: f64 = 10.0;
const BRIEF_CHIP_H: f64 = 22.0;
const BRIEF_CHIP_GAP: f64 = 6.0;
const BRIEF_CHIP_ROW_GAP: f64 = 6.0;
const BRIEF_CHIP_PAD: f64 = 9.0;
/// The instruction field: 12 pt text, three lines at least, six then it
/// scrolls. `sidebar_view` insets the native field by the two pads.
pub(super) const BRIEF_FIELD_LINE_H: f64 = 20.0;
pub(super) const BRIEF_FIELD_PAD_H: f64 = 8.0;
pub(super) const BRIEF_FIELD_PAD_V: f64 = 6.0;
const BRIEF_FIELD_MIN_LINES: usize = 3;
const BRIEF_FIELD_MAX_LINES: usize = 6;
const BRIEF_BTN_H: f64 = 26.0;
const BRIEF_BTN_GAP: f64 = 8.0;
const BRIEF_BTN_PAD: f64 = 12.0;
const BRIEF_SEND_ICON: f64 = 12.0;
const BRIEF_SEND_ICON_GAP: f64 = 4.0;
const BRIEF_HINT_H: f64 = 14.0;
/// The black the list takes behind the card.
pub(super) const BRIEF_SCRIM: f64 = 0.4;

// ---------------------------------------------------------------
// The view model
// ---------------------------------------------------------------

/// A permission prompt as its card shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionCard {
    /// The screen the card was built from. A click answers only while a fresh
    /// read of the pane hashes to the same `fingerprint`
    /// (`prompt_preview::permission_answer`).
    pub prompt: ParsedPrompt,
    /// `ParsedPrompt::fingerprint`, and the card id's last part.
    pub fingerprint: u64,
    /// The digit each button types; `None` leaves the button out.
    pub approve: Option<u32>,
    pub always: Option<u32>,
    pub reject: Option<u32>,
}

impl PermissionCard {
    pub fn new(prompt: ParsedPrompt) -> Self {
        let choices = prompt.choices();
        PermissionCard {
            fingerprint: prompt.fingerprint(),
            prompt,
            approve: choices.approve,
            always: choices.always,
            reject: choices.reject,
        }
    }
}

/// One question, or one waiting permission prompt, as the sidebar card shows
/// it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AskCardVm {
    /// A stable, cheap key for the hit enum: the question id hashed. The
    /// view can compare hits without cloning a string on every mouse move.
    pub key: u64,
    pub id: String,
    /// The agent or session that asked, or the pane whose prompt waits.
    pub from: String,
    /// The tab its pane belongs to, empty when the pane is gone.
    pub tab: String,
    /// The pane to focus from the `open` ghost button, when it still exists.
    pub pane_id: Option<PaneId>,
    /// `4m`, `2h`.
    pub age: String,
    /// Two hours or more: the age takes the warn colour.
    pub urgent: bool,
    pub message: String,
    /// The two options OK and KO send: the question's first two actions, or
    /// `OK` / `No` when it declared fewer. Empty on a permission card.
    pub options: [String; 2],
    /// Every option the agent proposed, in order: the Brief overlay's chips.
    /// One `Reply` when it proposed none, like the card's single button.
    pub choices: Vec<String>,
    pub created_at: u64,
    /// The question's conversation has no live pane in Kova and no live
    /// Claude process: the note says `Session ended`.
    pub ended: bool,
    /// Put off until `back_at`.
    pub snoozed: bool,
    /// `20:00`, local time.
    pub back_at: String,
    /// A permission prompt read off a pane's screen: this card replaces the
    /// question's, with its own buttons and no Brief, Snooze or note.
    pub permission: Option<PermissionCard>,
    /// A decision sheet (`kova ask --decision`): a `Decision` tag, Open (the
    /// sheet in the side panel) in place of KO, OK answers Done.
    pub decision_url: Option<String>,
    /// A handoff's step (`Ouvrir`, `Lancer`, `crate::handoff`): OK performs it
    /// without answering, so the card stays as it is instead of `Sending`.
    pub step: Option<String>,
}

/// The key the hit enum carries for a question id.
pub fn ask_key(id: &str) -> u64 {
    let mut h = DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

impl AskCardVm {
    /// The card at `now`, from the board's question and the pane the asker
    /// still runs in (`None` once Kova no longer has it). `live` says the
    /// conversation is alive in another process, so its session has not
    /// ended even without a pane here.
    pub fn from_question(question: &crate::ask::Question, pane: Option<(PaneId, String)>, live: bool, now: u64) -> Self {
        let choices =
            if question.actions.is_empty() { vec!["Reply".to_string()] } else { question.actions.clone() };
        let first = choices[0].clone();
        let second = choices.get(1).cloned().unwrap_or_else(|| "No".to_string());
        // A handoff: the step stays off the Brief's chips (it answers nothing,
        // an instruction on it would be lost), and the card shows what it
        // opens or runs, as the floating card does.
        let step = crate::handoff::step_button(question);
        let choices: Vec<String> = choices.into_iter().filter(|c| Some(c.as_str()) != step).collect();
        let message = match crate::handoff::detail(question).map(|d| d.card_text()).filter(|d| !d.is_empty()) {
            Some(detail) => format!("{}\n\n{}", question.message, detail),
            None => question.message.clone(),
        };
        let age_secs = now.saturating_sub(question.asked_at);
        let snoozed = question.snoozed_until > now;
        AskCardVm {
            key: ask_key(&question.id),
            id: question.id.clone(),
            from: question.from.name.clone(),
            tab: pane.as_ref().map(|(_, tab)| tab.clone()).unwrap_or_default(),
            pane_id: pane.as_ref().map(|(id, _)| *id),
            age: format_age(age_secs),
            urgent: age_secs >= URGENT_AFTER_SECS,
            message,
            options: [first, second],
            choices,
            created_at: question.asked_at,
            ended: pane.is_none() && !live,
            snoozed,
            back_at: if snoozed { local_hhmm(question.snoozed_until) } else { String::new() },
            permission: None,
            decision_url: question.panel_link(),
            step: step.map(String::from),
        }
    }

    /// The card of the permission prompt read in `pane`: its question is the
    /// message, the command goes in the code block, and the id carries the
    /// screen's fingerprint, so a prompt that changed is another card. `from`
    /// is the pane's agent session name, else its title.
    pub fn from_permission(prompt: ParsedPrompt, pane: (PaneId, String), from: String, since: u64, now: u64) -> Self {
        let permission = PermissionCard::new(prompt);
        let age_secs = now.saturating_sub(since);
        let id = format!("perm:{}:{:016x}", pane.0, permission.fingerprint);
        AskCardVm {
            key: ask_key(&id),
            id,
            from,
            tab: pane.1,
            pane_id: Some(pane.0),
            age: format_age(age_secs),
            urgent: age_secs >= URGENT_AFTER_SECS,
            message: permission.prompt.question.clone(),
            options: [String::new(), String::new()],
            choices: Vec::new(),
            created_at: since,
            ended: false,
            snoozed: false,
            back_at: String::new(),
            permission: Some(permission),
            decision_url: None,
            step: None,
        }
    }
}

/// What the view keeps between ticks: the cards read from the board and the
/// panes, the folded state of the snoozed section, the card whose snooze row
/// is open, the card being sent, and the highlighted card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AsksState {
    pub cards: Vec<AskCardVm>,
    pub folded: bool,
    /// The question id whose snooze row is open.
    pub snooze_open: Option<String>,
    pub transient: Option<TransientAsk>,
    pub highlight: Option<Highlight>,
    /// A permission card whose click found another screen: the pane says so
    /// on its card, in crit, for `FLASH_MS`.
    pub flash: Option<Flash>,
    /// The permission card the next read of the list lights: a need row's
    /// `Answer` names the pane, and the card's id carries the screen's
    /// fingerprint, so it is resolved once the cards are read.
    pub highlight_pane: Option<PaneId>,
    /// The Brief overlay, when one is open.
    pub brief: Option<BriefOpen>,
    /// What was typed in the Brief field, by question id. Kept after the
    /// overlay closes, so reopening Brief on the same ask restores it.
    pub drafts: HashMap<String, String>,
}

impl Default for AsksState {
    fn default() -> Self {
        AsksState {
            cards: Vec::new(),
            folded: true,
            snooze_open: None,
            transient: None,
            highlight: None,
            flash: None,
            highlight_pane: None,
            brief: None,
            drafts: HashMap::new(),
        }
    }
}

impl AsksState {
    pub fn open(&self) -> impl Iterator<Item = &AskCardVm> {
        self.cards.iter().filter(|card| !card.snoozed)
    }

    pub fn snoozed(&self) -> impl Iterator<Item = &AskCardVm> {
        self.cards.iter().filter(|card| card.snoozed)
    }

    pub fn open_count(&self) -> usize {
        self.open().count()
    }

    pub fn card(&self, key: u64) -> Option<&AskCardVm> {
        self.cards.iter().find(|card| card.key == key)
    }

    /// A new read of the board and the panes. The transient answer survives
    /// it: it is the card that just left the list (or is about to). The Brief
    /// overlay closes with its question, keeping the draft.
    pub fn set_cards(&mut self, cards: Vec<AskCardVm>) {
        self.cards = cards;
        if let Some(brief) = &self.brief
            && !self.cards.iter().any(|card| card.id == brief.id)
        {
            self.brief = None;
        }
    }

    /// Drop the transient card once the list no longer holds it (it becomes
    /// `Sent`, for `SENT_LINGER_MS`) or once a sending click gave up. Clear a
    /// highlight or a crit line that ran out. `waiting` is the keys the list
    /// still holds: the board's questions and the panes' prompts.
    pub fn prune(&mut self, waiting: &[u64], now_ms: u64) {
        if let Some(t) = &mut self.transient {
            match t.phase {
                TransientPhase::Sending => {
                    if !waiting.contains(&t.card.key) {
                        t.phase = TransientPhase::Sent;
                        t.until_ms = now_ms + SENT_LINGER_MS;
                    } else if now_ms >= t.until_ms {
                        self.transient = None;
                    }
                }
                TransientPhase::Sent if now_ms >= t.until_ms => self.transient = None,
                TransientPhase::Sent => {}
            }
        }
        if self.highlight.as_ref().is_some_and(|h| now_ms >= h.until_ms) {
            self.highlight = None;
        }
        if self.flash.as_ref().is_some_and(|f| now_ms >= f.until_ms) {
            self.flash = None;
        }
    }
}

/// A card whose answer is on its way, or was just handed to the board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransientAsk {
    pub card: AskCardVm,
    pub phase: TransientPhase,
    /// Epoch millis: the sending timeout, then the sent linger's end.
    pub until_ms: u64,
}

/// The Brief overlay: an instruction typed on top of one question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BriefOpen {
    pub id: String,
    /// The chip picked, as an index into the card's `choices`; `None` sends
    /// the free answer (`ask::OTHER`) instead.
    pub picked: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransientPhase {
    Sending,
    Sent,
}

/// The card a need row walked to, lit for `HIGHLIGHT_MS`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Highlight {
    pub id: String,
    pub until_ms: u64,
}

/// The crit line a refused permission click leaves on its card: the screen
/// changed under it, so the pane has to be opened instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flash {
    pub pane_id: PaneId,
    pub until_ms: u64,
}

// ---------------------------------------------------------------
// Actions
// ---------------------------------------------------------------

/// What a click asks the board to do, queued on the delegate and served by
/// the main loop (`app.rs`), like the floating card's own answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AskAction {
    Answer { id: String, action: String, text: String },
    Snooze { id: String, until: u64 },
    Unsnooze { id: String },
}

/// The queue between the click and the tick that owns the board.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AskQueue {
    actions: Vec<AskAction>,
}

impl AskQueue {
    pub fn push(&mut self, action: AskAction) {
        self.actions.push(action);
    }

    /// Take everything queued. The caller serves it on the main loop.
    pub fn drain(&mut self) -> Vec<AskAction> {
        std::mem::take(&mut self.actions)
    }

    pub fn len(&self) -> usize {
        self.actions.len()
    }
}

/// The three snooze choices, as the card offers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnoozeChoice {
    OneHour,
    Tonight,
    Tomorrow,
}

impl SnoozeChoice {
    pub const ALL: [SnoozeChoice; 3] = [SnoozeChoice::OneHour, SnoozeChoice::Tonight, SnoozeChoice::Tomorrow];

    pub fn label(self) -> &'static str {
        match self {
            SnoozeChoice::OneHour => "1 h",
            SnoozeChoice::Tonight => "Tonight 20:00",
            SnoozeChoice::Tomorrow => "Tomorrow 9:00",
        }
    }

    /// When the question comes back. Local wall clock, clamped to the
    /// board's 24 h bound like every other snooze.
    pub fn until(self, now: u64) -> u64 {
        let raw = match self {
            SnoozeChoice::OneHour => now + 3600,
            SnoozeChoice::Tonight => {
                let tonight = local_epoch(now, 0, 20, 0);
                if tonight > now { tonight } else { local_epoch(now, 1, 20, 0) }
            }
            SnoozeChoice::Tomorrow => local_epoch(now, 1, 9, 0),
        };
        raw.min(now + crate::ask::SNOOZE_MAX_SECS).max(now + 1)
    }
}

// ---------------------------------------------------------------
// Hit targets
// ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsksHit {
    /// Answer with the first option.
    Ok(u64),
    /// Answer with the second option (or `No`).
    Ko(u64),
    /// Open the Brief overlay on this question.
    Brief(u64),
    /// Pick a chip of the open Brief overlay, or unpick it when it already is.
    BriefChip(u64, usize),
    /// Send the instruction the Brief overlay holds.
    BriefSend(u64),
    /// Close the Brief overlay without sending. A click outside its card is
    /// this too.
    BriefCancel(u64),
    /// Focus the pane the question came from.
    Open(PaneId),
    /// Show a decision card's sheet in the side panel. Does not answer.
    OpenDoc(u64),
    /// Open or close the three-choice snooze row.
    SnoozeMenu(u64),
    Snooze(u64, SnoozeChoice),
    /// Fold or unfold the `Snoozed N` section.
    ToggleSnoozed,
    /// Cancel a snooze: the question is open now.
    ShowNow(u64),
    /// A permission prompt: type the option each label names, if the screen
    /// still is the one the card shows.
    PermissionApprove(u64),
    PermissionReject(u64),
    PermissionAlways(u64),
}

impl AsksHit {
    /// The permission choice a hit is, with the card key. `None` for every
    /// other button: those go through `click`.
    pub fn permission(self) -> Option<(u64, PermissionChoice)> {
        match self {
            AsksHit::PermissionApprove(key) => Some((key, PermissionChoice::Approve)),
            AsksHit::PermissionReject(key) => Some((key, PermissionChoice::Reject)),
            AsksHit::PermissionAlways(key) => Some((key, PermissionChoice::Always)),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------
// Clicks
// ---------------------------------------------------------------

/// What a click does to the view state, and the action to queue. `now_ms` is
/// epoch milliseconds. A card already being sent ignores further clicks, so a
/// double click answers once.
pub fn click(state: &mut AsksState, hit: AsksHit, now_ms: u64) -> Option<AskAction> {
    match hit {
        AsksHit::Ok(key) => answer(state, key, 0, now_ms),
        AsksHit::Ko(key) => answer(state, key, 1, now_ms),
        AsksHit::Brief(key) => {
            let id = state.card(key)?.id.clone();
            state.snooze_open = None;
            state.brief = Some(BriefOpen { id, picked: None });
            None
        }
        AsksHit::BriefChip(key, index) => {
            let card = state.card(key)?;
            if index >= card.choices.len() {
                return None;
            }
            let id = card.id.clone();
            let brief = state.brief.as_mut().filter(|brief| brief.id == id)?;
            brief.picked = if brief.picked == Some(index) { None } else { Some(index) };
            None
        }
        AsksHit::BriefSend(key) => send_brief(state, key, now_ms),
        AsksHit::BriefCancel(key) => {
            let id = state.card(key)?.id.clone();
            if state.brief.as_ref().is_some_and(|brief| brief.id == id) {
                state.brief = None;
            }
            None
        }
        AsksHit::Open(_) | AsksHit::OpenDoc(_) => None,
        AsksHit::SnoozeMenu(key) => {
            let id = state.card(key)?.id.clone();
            state.snooze_open = if state.snooze_open.as_deref() == Some(id.as_str()) { None } else { Some(id) };
            None
        }
        AsksHit::Snooze(key, choice) => {
            let id = state.card(key)?.id.clone();
            state.snooze_open = None;
            Some(AskAction::Snooze { id, until: choice.until(now_ms / 1000) })
        }
        AsksHit::ToggleSnoozed => {
            state.folded = !state.folded;
            None
        }
        AsksHit::ShowNow(key) => state.card(key).map(|card| AskAction::Unsnooze { id: card.id.clone() }),
        // A permission button is served by `sidebar_ui::sidebar_ask_hit`,
        // which holds the pane: only a click that really reached the PTY
        // moves the card (`permission_sending`).
        AsksHit::PermissionApprove(_) | AsksHit::PermissionReject(_) | AsksHit::PermissionAlways(_) => None,
    }
}

fn answer(state: &mut AsksState, key: u64, option: usize, now_ms: u64) -> Option<AskAction> {
    if state.transient.as_ref().is_some_and(|t| t.card.key == key) {
        return None;
    }
    let card = state.card(key)?.clone();
    // A handoff's step leaves the question open: nothing to send.
    if card.step.as_ref() != Some(&card.options[option]) {
        state.transient = Some(TransientAsk {
            card: card.clone(),
            phase: TransientPhase::Sending,
            until_ms: now_ms + SENDING_TIMEOUT_MS,
        });
    }
    Some(AskAction::Answer { id: card.id, action: card.options[option].clone(), text: String::new() })
}

/// Queue the Brief's answer: the picked chip, or the free answer `Other` when
/// none is. The text is trimmed and capped at `ask::ANSWER_TEXT_MAX`; with no
/// chip and no text there is nothing to send and the overlay stays open.
fn send_brief(state: &mut AsksState, key: u64, now_ms: u64) -> Option<AskAction> {
    if state.transient.as_ref().is_some_and(|t| t.card.key == key) {
        return None;
    }
    let card = state.card(key)?.clone();
    let brief = state.brief.as_ref().filter(|brief| brief.id == card.id)?;
    let draft = state.drafts.get(&card.id).map(String::as_str).unwrap_or("");
    let action = brief_action(&card, brief.picked, draft)?;
    state.brief = None;
    state.drafts.remove(&card.id);
    state.transient = Some(TransientAsk {
        card: card.clone(),
        phase: TransientPhase::Sending,
        until_ms: now_ms + SENDING_TIMEOUT_MS,
    });
    Some(action)
}

/// A permission click whose digit reached the pane: the card goes to
/// `Sending`, with the same timeout an ask answer has, and disappears when the
/// prompt leaves the screen. Nothing happens on a card already being sent.
pub fn permission_sending(state: &mut AsksState, key: u64, now_ms: u64) {
    if state.transient.as_ref().is_some_and(|t| t.card.key == key) {
        return;
    }
    let Some(card) = state.card(key).cloned() else { return };
    if state.flash.as_ref().is_some_and(|f| Some(f.pane_id) == card.pane_id) {
        state.flash = None;
    }
    state.transient = Some(TransientAsk {
        card,
        phase: TransientPhase::Sending,
        until_ms: now_ms + SENDING_TIMEOUT_MS,
    });
}

/// The screen moved on under a permission card: nothing was typed, and the
/// card says why in crit for `FLASH_MS`.
pub fn permission_refused(state: &mut AsksState, pane_id: PaneId, now_ms: u64) {
    state.flash = Some(Flash { pane_id, until_ms: now_ms + FLASH_MS });
}

/// What the Brief sends: `picked` is an index into `card.choices`, `draft` the
/// field's text. No chip means the free answer, so an empty draft has nothing
/// to send. The text is trimmed, then cut at `ask::ANSWER_TEXT_MAX` bytes on a
/// character boundary.
pub fn brief_action(card: &AskCardVm, picked: Option<usize>, draft: &str) -> Option<AskAction> {
    let text = cap_answer_text(draft.trim());
    let action = match picked.and_then(|i| card.choices.get(i)) {
        Some(choice) => choice.clone(),
        None if !text.is_empty() => crate::ask::OTHER.to_string(),
        None => return None,
    };
    Some(AskAction::Answer { id: card.id.clone(), action, text: text.to_string() })
}

fn cap_answer_text(text: &str) -> &str {
    if text.len() <= crate::ask::ANSWER_TEXT_MAX {
        return text;
    }
    let mut end = crate::ask::ANSWER_TEXT_MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

// ---------------------------------------------------------------
// Layout
// ---------------------------------------------------------------

/// One card's geometry, in list coordinates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CardLayout {
    pub card: AskCardVm,
    pub frame: Rect,
    pub header: Rect,
    pub message: Rect,
    pub message_lines: usize,
    /// The `Permission` tag in a permission card's header.
    pub tag: Option<Rect>,
    /// The command block under a permission card's question, and the lines
    /// the block shows (already wrapped, at most `CODE_MAX_LINES`).
    pub code: Option<Rect>,
    pub code_lines: Vec<String>,
    /// The crit line of a refused permission click.
    pub flash: Option<Rect>,
    /// The sending/sent line, in the action row's place.
    pub status: Option<Rect>,
    pub phase: Option<TransientPhase>,
    /// Every clickable box, with its hit.
    pub buttons: Vec<(AsksHit, Rect)>,
    /// The note line under the actions: options and `Session ended`, or the
    /// `back at` footer of a snoozed card.
    pub note: Rect,
    pub snooze_row: Option<[Rect; 3]>,
    pub highlighted: bool,
    /// A snoozed card: drawn dimmed, without actions.
    pub faded: bool,
}

/// One chip of the Brief overlay: the option it sends, its label.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BriefChip {
    pub index: usize,
    pub label: String,
    pub rect: Rect,
}

/// The Brief overlay, laid out: one card over the list, and what it holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BriefLayout {
    /// The question briefed: its key and id.
    pub key: u64,
    pub id: String,
    pub from: String,
    pub age: String,
    pub card: Rect,
    pub header: Rect,
    pub message: String,
    pub message_rect: Rect,
    /// The options, as chips, in the order they were proposed.
    pub choices: Vec<BriefChip>,
    pub picked: Option<usize>,
    /// The instruction field, where the native field sits.
    pub field: Rect,
    pub cancel: Rect,
    pub send: Rect,
    pub hint: Rect,
    /// A send has something to send: an option or some text.
    pub send_enabled: bool,
}

/// One object of the Asks list, laid out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AsksLayout {
    /// The list is up (the model's view is Asks).
    pub shown: bool,
    pub cards: Vec<CardLayout>,
    pub snoozed_header: Option<Rect>,
    pub snoozed_count: usize,
    pub snoozed_cards: Vec<CardLayout>,
    pub empty: Option<Rect>,
    /// The Brief overlay, when a question is being briefed. It covers the
    /// list: nothing under it is clickable.
    pub brief: Option<BriefLayout>,
    pub content_h: f64,
}

impl AsksLayout {
    /// The button under `(x, y)`, if any. A press on the card's body is not a
    /// target: nothing happens there.
    pub fn hit(&self, x: f64, y: f64) -> Option<AsksHit> {
        if let Some(brief) = &self.brief {
            if !brief.card.contains(x, y) {
                return Some(AsksHit::BriefCancel(brief.key));
            }
            if let Some(chip) = brief.choices.iter().find(|chip| chip.rect.contains(x, y)) {
                return Some(AsksHit::BriefChip(brief.key, chip.index));
            }
            if brief.cancel.contains(x, y) {
                return Some(AsksHit::BriefCancel(brief.key));
            }
            if brief.send.contains(x, y) {
                return Some(AsksHit::BriefSend(brief.key));
            }
            // The field is a native subview and sees its own clicks; the
            // rest of the card is text.
            return None;
        }
        for card in self.cards.iter().chain(self.snoozed_cards.iter()) {
            if !card.frame.contains(x, y) {
                continue;
            }
            return card.buttons.iter().find(|(_, rect)| rect.contains(x, y)).map(|(hit, _)| *hit);
        }
        if self.snoozed_header.is_some_and(|rect| rect.contains(x, y)) {
            return Some(AsksHit::ToggleSnoozed);
        }
        None
    }

    /// Every clickable box, for the hand cursor.
    pub fn button_rects(&self) -> Vec<Rect> {
        if let Some(brief) = &self.brief {
            let mut rects: Vec<Rect> = brief.choices.iter().map(|chip| chip.rect).collect();
            rects.push(brief.cancel);
            if brief.send_enabled {
                rects.push(brief.send);
            }
            return rects;
        }
        let mut rects: Vec<Rect> = self
            .cards
            .iter()
            .chain(self.snoozed_cards.iter())
            .flat_map(|card| card.buttons.iter().map(|(_, rect)| *rect))
            .collect();
        rects.extend(self.snoozed_header);
        rects
    }

    pub fn card_frame(&self, id: &str) -> Option<Rect> {
        self.cards
            .iter()
            .chain(self.snoozed_cards.iter())
            .find(|card| card.card.id == id)
            .map(|card| card.frame)
    }
}

/// Lay the Asks list out at `width`. The open cards come newest first, then
/// the folded or open `Snoozed N` section.
pub fn layout(state: &AsksState, width: f64, m: &dyn TextMetrics) -> AsksLayout {
    let x0 = PAD_H;
    let x1 = (width - (PAD_H - EDGE_TOLERANCE)).max(x0 + 40.0);
    let w = x1 - x0;
    let mut y = LIST_TOP;
    let mut out = AsksLayout { shown: true, ..Default::default() };
    if state.cards.is_empty() && state.transient.is_none() {
        out.empty = Some(Rect::new(x0, y, w, EMPTY_H));
        out.content_h = y + EMPTY_H + LIST_BOTTOM;
        return out;
    }

    let mut open: Vec<&AskCardVm> = state.open().collect();
    if let Some(t) = &state.transient
        && !open.iter().any(|card| card.key == t.card.key)
    {
        open.push(&t.card);
    }
    open.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| a.id.cmp(&b.id)));
    for card in open {
        let phase = state.transient.as_ref().filter(|t| t.card.key == card.key).map(|t| t.phase);
        let (mut row, h) = open_card(card, phase, state, x0, y, w, m);
        row.frame.h = h;
        row.highlighted = state.highlight.as_ref().is_some_and(|high| high.id == card.id);
        out.cards.push(row);
        y += h + CARD_GAP;
    }

    let mut snoozed: Vec<&AskCardVm> = state.snoozed().collect();
    snoozed.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| a.id.cmp(&b.id)));
    if !snoozed.is_empty() {
        out.snoozed_count = snoozed.len();
        out.snoozed_header = Some(Rect::new(x0, y, w, SNOOZE_HEADER_H));
        y += SNOOZE_HEADER_H;
        if !state.folded {
            y += SNOOZE_CONTENT_GAP;
            for card in snoozed {
                let (row, h) = snoozed_card(card, x0, y, w, m);
                out.snoozed_cards.push(row);
                y += h + CARD_GAP;
            }
        }
    }
    out.brief = brief_layout(state, width, m);
    out.content_h = y + LIST_BOTTOM;
    if let Some(brief) = &out.brief {
        // The card may be taller than the list: the list still scrolls to
        // its bottom, so all of it can be read.
        out.content_h = out.content_h.max(brief.card.bottom() + LIST_BOTTOM);
    }
    out
}

/// The Brief overlay's geometry at `width`: a card centred at the top of the
/// list. `None` when no overlay is open, or when the question it names left
/// the list (answered elsewhere).
fn brief_layout(state: &AsksState, width: f64, m: &dyn TextMetrics) -> Option<BriefLayout> {
    let brief = state.brief.as_ref()?;
    let card = state.cards.iter().find(|card| card.id == brief.id)?;
    let x = BRIEF_SIDE;
    let w = (width - 2.0 * BRIEF_SIDE).max(60.0);
    let y = LIST_TOP + BRIEF_TOP;
    let inner_x = x + BRIEF_PAD;
    let inner_w = (w - 2.0 * BRIEF_PAD).max(0.0);

    let header = Rect::new(inner_x, y + BRIEF_PAD, inner_w, HEADER_H);
    let message_lines = m.lines(&card.message, Style::Secondary, inner_w, MSG_MAX_LINES).max(1);
    let message = Rect::new(inner_x, header.bottom() + HEADER_GAP, inner_w, message_lines as f64 * MSG_LINE_H);

    let mut choices = Vec::new();
    let mut cx = inner_x;
    let mut cy = message.bottom() + BRIEF_GAP;
    for (index, label) in card.choices.iter().enumerate() {
        let cw = (m.width(label, Style::Chip) + 2.0 * BRIEF_CHIP_PAD).min(inner_w);
        if cx > inner_x && cx + cw > inner_x + inner_w {
            cx = inner_x;
            cy += BRIEF_CHIP_H + BRIEF_CHIP_ROW_GAP;
        }
        choices.push(BriefChip { index, label: label.clone(), rect: Rect::new(cx, cy, cw, BRIEF_CHIP_H) });
        cx += cw + BRIEF_CHIP_GAP;
    }
    let chips_bottom = if choices.is_empty() { cy } else { cy + BRIEF_CHIP_H };

    let draft = state.drafts.get(&card.id).map(String::as_str).unwrap_or("");
    let field_w = inner_w.max(1.0);
    let text_w = (field_w - 2.0 * BRIEF_FIELD_PAD_H).max(1.0);
    let lines = m
        .lines(draft, Style::Secondary, text_w, BRIEF_FIELD_MAX_LINES)
        .clamp(BRIEF_FIELD_MIN_LINES, BRIEF_FIELD_MAX_LINES);
    let field = Rect::new(inner_x, chips_bottom + BRIEF_GAP, field_w, lines as f64 * BRIEF_FIELD_LINE_H);

    let send_w = m.width("Send", Style::Chip) + 2.0 * BRIEF_BTN_PAD + BRIEF_SEND_ICON + BRIEF_SEND_ICON_GAP;
    let cancel_w = m.width("Cancel", Style::Chip) + 2.0 * BRIEF_BTN_PAD;
    let buttons_y = field.bottom() + BRIEF_GAP;
    let send = Rect::new(inner_x + inner_w - send_w, buttons_y, send_w, BRIEF_BTN_H);
    let cancel = Rect::new(send.x - BRIEF_BTN_GAP - cancel_w, buttons_y, cancel_w, BRIEF_BTN_H);
    let hint = Rect::new(inner_x, buttons_y + BRIEF_BTN_H + HEADER_GAP, inner_w, 2.0 * BRIEF_HINT_H);

    let picked = brief.picked.filter(|index| *index < card.choices.len());
    Some(BriefLayout {
        key: card.key,
        id: card.id.clone(),
        from: card.from.clone(),
        age: card.age.clone(),
        card: Rect::new(x, y, w, hint.bottom() + BRIEF_PAD - y),
        header,
        message: card.message.clone(),
        message_rect: message,
        choices,
        picked,
        field,
        cancel,
        send,
        hint,
        send_enabled: picked.is_some() || !draft.trim().is_empty(),
    })
}

fn open_card(
    card: &AskCardVm,
    phase: Option<TransientPhase>,
    state: &AsksState,
    x: f64,
    y: f64,
    w: f64,
    m: &dyn TextMetrics,
) -> (CardLayout, f64) {
    if card.permission.is_some() {
        return permission_card(card, phase, state, x, y, w, m);
    }
    let inner_x = x + CARD_PAD_H;
    let inner_w = (w - 2.0 * CARD_PAD_H).max(0.0);
    let lines = m.lines(&card.message, Style::Secondary, inner_w, MSG_MAX_LINES).max(1);
    let header = Rect::new(inner_x, y + CARD_PAD_V, inner_w, HEADER_H);
    let message = Rect::new(inner_x, header.bottom() + HEADER_GAP, inner_w, lines as f64 * MSG_LINE_H);
    let mut next = message.bottom() + ACT_GAP;
    let mut buttons = Vec::new();
    let mut snooze_row = None;
    let mut note = Rect::default();
    let status = if phase.is_some() {
        let status = Rect::new(inner_x, next, inner_w, STATUS_H);
        next += STATUS_H;
        Some(status)
    } else {
        let action_y = next;
        if card.decision_url.is_some() {
            // A decision: Open the sheet first (it does not answer), then Done
            // (OK, the first action) and Brief. No KO: the sheet is the answer.
            let open = Rect::new(inner_x, action_y, m.width(OPEN_DOC_TEXT, Style::Chip) + 2.0 * ALWAYS_PAD + 8.0, AB_H);
            let ok = Rect::new(open.right() + AB_GAP, action_y, AB_W, AB_H);
            let brief = Rect::new(ok.right() + AB_GAP, action_y, AB_W, AB_H);
            buttons.push((AsksHit::OpenDoc(card.key), open));
            buttons.push((AsksHit::Ok(card.key), ok));
            buttons.push((AsksHit::Brief(card.key), brief));
        } else {
            let ok = Rect::new(inner_x, action_y, AB_W, AB_H);
            let ko = Rect::new(ok.right() + AB_GAP, action_y, AB_W, AB_H);
            let brief = Rect::new(ko.right() + AB_GAP, action_y, AB_W, AB_H);
            buttons.push((AsksHit::Ok(card.key), ok));
            buttons.push((AsksHit::Ko(card.key), ko));
            buttons.push((AsksHit::Brief(card.key), brief));
        }
        let moon = Rect::new(inner_x + inner_w - GHOST_W, action_y, GHOST_W, AB_H);
        buttons.push((AsksHit::SnoozeMenu(card.key), moon));
        if let Some(pane_id) = card.pane_id {
            let open = Rect::new(moon.x - GHOST_GAP - GHOST_W, action_y, GHOST_W, AB_H);
            buttons.push((AsksHit::Open(pane_id), open));
        }
        next += AB_H;
        if state.snooze_open.as_deref() == Some(card.id.as_str()) {
            next += SNOOZE_ROW_GAP;
            let labels = SnoozeChoice::ALL;
            let natural: f64 =
                labels.iter().map(|c| m.width(c.label(), Style::Chip) + 2.0 * SNOOZE_BTN_PAD).sum::<f64>()
                    + SNOOZE_BTN_GAP * 2.0;
            let share = (inner_w - 2.0 * SNOOZE_BTN_GAP).max(0.0) / 3.0;
            let mut rects = [Rect::default(); 3];
            let mut bx = inner_x;
            for (i, choice) in labels.iter().enumerate() {
                let bw = if natural > inner_w {
                    share
                } else {
                    m.width(choice.label(), Style::Chip) + 2.0 * SNOOZE_BTN_PAD
                };
                let rect = Rect::new(bx, next, bw, SNOOZE_BTN_H);
                rects[i] = rect;
                buttons.push((AsksHit::Snooze(card.key, *choice), rect));
                bx += bw + SNOOZE_BTN_GAP;
            }
            snooze_row = Some(rects);
            next += SNOOZE_BTN_H;
        }
        next += NOTE_GAP;
        note = Rect::new(inner_x, next, inner_w, NOTE_H);
        next += NOTE_H;
        None
    };
    let h = next - y + CARD_PAD_V;
    (
        CardLayout {
            frame: Rect::new(x, y, w, h),
            header,
            message,
            message_lines: lines,
            tag: header_columns(card, header, m).1,
            code: None,
            code_lines: Vec::new(),
            flash: None,
            status,
            phase,
            buttons,
            note,
            snooze_row,
            highlighted: false,
            faded: false,
            card: card.clone(),
        },
        h,
    )
}

/// The header's columns after the dot, shared by the layout and the drawing so
/// the tag and the tab beside it cannot drift: `from`'s width, the tag, the
/// tab's width.
fn header_columns(card: &AskCardVm, header: Rect, m: &dyn TextMetrics) -> (f64, Option<Rect>, f64) {
    let age_w = m.width(&card.age, Style::Number);
    let text_x = header.x + HEADER_TEXT_X;
    let room = (header.right() - age_w - 8.0 - text_x).max(0.0);
    let tag_w = match tag_text(card) {
        Some(text) => m.width(text, Style::Hint) + 2.0 * TAG_PAD_H,
        None => 0.0,
    };
    let tab_w = if card.tab.is_empty() { 0.0 } else { m.width(&card.tab, Style::Summary).min(room * 0.45) };
    let gap = if tag_w > 0.0 { HEADER_TAB_GAP } else { 0.0 };
    let tab_gap = if tab_w > 0.0 { HEADER_TAB_GAP } else { 0.0 };
    let from_w = (room - tag_w - gap - tab_w - tab_gap).max(0.0);
    let tag = (tag_w > 0.0).then(|| Rect::new(text_x + from_w + gap, header.y + (HEADER_H - TAG_H) / 2.0, tag_w, TAG_H));
    (from_w, tag, tab_w)
}

/// The tag a card's header carries: `Permission`, `Decision`, or none.
fn tag_text(card: &AskCardVm) -> Option<&'static str> {
    if card.permission.is_some() {
        Some(TAG_TEXT)
    } else if card.decision_url.is_some() {
        Some(DECISION_TAG_TEXT)
    } else {
        None
    }
}

/// The code block's lines: hard-wrapped at Menlo 11's advance, at most
/// `CODE_MAX_LINES`, the last one cut with an ellipsis when there is more.
fn code_lines(text: &str, width: f64) -> Vec<String> {
    let cols = (((width - 2.0 * CODE_PAD) / CODE_ADVANCE).floor().max(1.0)) as usize;
    let mut out: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let mut current = String::new();
        let mut count = 0;
        for c in line.chars() {
            if count == cols {
                out.push(std::mem::take(&mut current));
                count = 0;
            }
            current.push(c);
            count += 1;
        }
        out.push(current);
    }
    if out.len() > CODE_MAX_LINES {
        out.truncate(CODE_MAX_LINES);
        if let Some(last) = out.last_mut() {
            *last = last.chars().take(cols.saturating_sub(1)).collect();
            last.push('\u{2026}');
        }
    }
    out
}

/// The card of a permission prompt: the head with its `Permission` tag, the
/// question, the command in its code block, then Approve, Reject and Always
/// where the screen offers them, and the ghost that opens the pane. No Brief,
/// no snooze and no note line: only what the prompt itself allows.
fn permission_card(
    card: &AskCardVm,
    phase: Option<TransientPhase>,
    state: &AsksState,
    x: f64,
    y: f64,
    w: f64,
    m: &dyn TextMetrics,
) -> (CardLayout, f64) {
    let inner_x = x + CARD_PAD_H;
    let inner_w = (w - 2.0 * CARD_PAD_H).max(0.0);
    let header = Rect::new(inner_x, y + CARD_PAD_V, inner_w, HEADER_H);
    let (_, tag, _) = header_columns(card, header, m);
    let lines = m.lines(&card.message, Style::Secondary, inner_w, MSG_MAX_LINES).max(1);
    let message = Rect::new(inner_x, header.bottom() + HEADER_GAP, inner_w, lines as f64 * MSG_LINE_H);

    let mut next = message.bottom();
    let mut code = None;
    let mut code_text = Vec::new();
    if let Some(command) = card.permission.as_ref().and_then(|p| p.prompt.detail.as_deref()).filter(|c| !c.is_empty())
    {
        code_text = code_lines(command, inner_w);
        let block = Rect::new(inner_x, next + CODE_GAP, inner_w, code_text.len() as f64 * CODE_LINE_H + 2.0 * CODE_PAD);
        next = block.bottom();
        code = Some(block);
    }

    let action_y = next + ACT_GAP;
    let mut buttons = Vec::new();
    let mut status = None;
    if phase.is_some() {
        status = Some(Rect::new(inner_x, action_y, inner_w, STATUS_H));
        next = action_y + STATUS_H;
    } else {
        let mut bx = inner_x;
        if let Some(permission) = &card.permission {
            if permission.approve.is_some() {
                buttons.push((AsksHit::PermissionApprove(card.key), Rect::new(bx, action_y, AB_W, AB_H)));
                bx += AB_W + AB_GAP;
            }
            if permission.reject.is_some() {
                buttons.push((AsksHit::PermissionReject(card.key), Rect::new(bx, action_y, AB_W, AB_H)));
                bx += AB_W + AB_GAP;
            }
            if permission.always.is_some() {
                let room = (inner_x + inner_w - bx).max(0.0);
                let bw = (m.width("Always", Style::Chip) + 2.0 * ALWAYS_PAD).min(room);
                buttons.push((AsksHit::PermissionAlways(card.key), Rect::new(bx, action_y, bw, AB_H)));
            }
        }
        if let Some(pane_id) = card.pane_id {
            let open = Rect::new(inner_x + inner_w - GHOST_W, action_y, GHOST_W, AB_H);
            buttons.push((AsksHit::Open(pane_id), open));
        }
        next = action_y + AB_H;
    }
    let flash = if state.flash.as_ref().is_some_and(|f| card.pane_id == Some(f.pane_id)) {
        let line = Rect::new(inner_x, next + NOTE_GAP, inner_w, NOTE_H);
        next = line.bottom();
        Some(line)
    } else {
        None
    };
    let h = next - y + CARD_PAD_V;
    (
        CardLayout {
            frame: Rect::new(x, y, w, h),
            header,
            message,
            message_lines: lines,
            tag,
            code,
            code_lines: code_text,
            flash,
            status,
            phase,
            buttons,
            note: Rect::default(),
            snooze_row: None,
            highlighted: false,
            faded: false,
            card: card.clone(),
        },
        h,
    )
}

fn snoozed_card(card: &AskCardVm, x: f64, y: f64, w: f64, m: &dyn TextMetrics) -> (CardLayout, f64) {
    let inner_x = x + CARD_PAD_H;
    let inner_w = (w - 2.0 * CARD_PAD_H).max(0.0);
    let lines = m.lines(&card.message, Style::Secondary, inner_w, MSG_MAX_LINES).max(1);
    let header = Rect::new(inner_x, y + CARD_PAD_V, inner_w, HEADER_H);
    let message = Rect::new(inner_x, header.bottom() + HEADER_GAP, inner_w, lines as f64 * MSG_LINE_H);
    let foot_y = message.bottom() + ACT_GAP;
    let show_w = (m.width("Show now", Style::Chip) + 2.0 * SHOW_NOW_PAD).min(inner_w);
    let show = Rect::new(inner_x + inner_w - show_w, foot_y, show_w, FOOT_H);
    let note = Rect::new(inner_x, foot_y, (inner_w - show_w - 8.0).max(0.0), FOOT_H);
    let h = foot_y + FOOT_H - y + CARD_PAD_V;
    (
        CardLayout {
            frame: Rect::new(x, y, w, h),
            header,
            message,
            message_lines: lines,
            tag: None,
            code: None,
            code_lines: Vec::new(),
            flash: None,
            status: None,
            phase: None,
            buttons: vec![(AsksHit::ShowNow(card.key), show)],
            note,
            snooze_row: None,
            highlighted: false,
            faded: true,
            card: card.clone(),
        },
        h,
    )
}

// ---------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------

/// Draw the whole Asks list. The view calls this instead of its row loop
/// while the Asks list is up. `scrim` is the visible part of the list, the
/// ground the Brief overlay dims.
pub(super) fn draw(
    sh: &Shared,
    state: &AsksState,
    layout: &AsksLayout,
    hovered: ListHit,
    pressed: ListHit,
    scrim: Rect,
) {
    let m = AppKitMetrics(sh);
    if let Some(empty) = layout.empty {
        draw_empty(sh, &empty);
        return;
    }
    for card in &layout.cards {
        draw_card(sh, &m, card, hovered, pressed);
    }
    if let Some(header) = layout.snoozed_header {
        draw_snoozed_header(sh, &m, &header, layout.snoozed_count, state.folded, hovered, pressed);
    }
    for card in &layout.snoozed_cards {
        draw_card(sh, &m, card, hovered, pressed);
    }
    if let Some(brief) = &layout.brief {
        draw_overlay(sh, &m, brief, &scrim, hovered, pressed);
    }
}

fn draw_card(sh: &Shared, m: &AppKitMetrics<'_>, card: &CardLayout, hovered: ListHit, pressed: ListHit) {
    let alpha = if card.faded { 0.55 } else { 1.0 };
    fill_round(&card.frame, CARD_RADIUS, tokens::get().tile, alpha);
    let (border, width) =
        if card.highlighted { (tokens::get().sel_rail, 1.5) } else { (tokens::get().border_subtle, 1.0) };
    stroke_round(&card.frame, CARD_RADIUS, width, border, alpha);

    draw_header(sh, m, card, alpha);
    draw_wrapped(sh, &card.card.message, &card.message, Style::Secondary, tokens::get().text_primary, alpha);
    if let Some(block) = card.code {
        draw_code(&block, &card.code_lines, alpha);
    }
    if let Some(flash) = card.flash {
        draw_text(sh, FLASH_TEXT, &flash, Style::Hint, tokens::get().crit_text, alpha, Align::Left, false);
    }

    if let (Some(status), Some(phase)) = (card.status, card.phase) {
        let (icon, fg, text) = match phase {
            TransientPhase::Sending => (super::feather::Icon::Send, tokens::get().text_tertiary, format!("Sending to {}...", card.card.from)),
            TransientPhase::Sent => (super::feather::Icon::CheckCircle, tokens::get().ok_dot, format!("Sent to {}", card.card.from)),
        };
        let icon_rect = Rect::new(status.x, status.y + (STATUS_H - STATUS_ICON) / 2.0, STATUS_ICON, STATUS_ICON);
        stroke_icon(icon, &icon_rect, STATUS_ICON, fg, alpha);
        draw_text(
            sh,
            &text,
            &Rect::new(icon_rect.right() + 6.0, status.y, (status.right() - icon_rect.right() - 6.0).max(0.0), status.h),
            Style::Secondary,
            fg,
            alpha,
            Align::Left,
            false,
        );
    } else if !card.faded && card.card.permission.is_none() {
        draw_options(sh, m, card, alpha);
    }
    if card.faded {
        draw_text(
            sh,
            &format!("back at {}", card.card.back_at),
            &card.note,
            Style::Hint,
            tokens::get().text_tertiary,
            alpha,
            Align::Left,
            false,
        );
    }

    for (hit, rect) in &card.buttons {
        draw_button(sh, *hit, rect, alpha, hovered, pressed);
    }
}

/// The Brief overlay: the scrim over the list, then the card. The instruction
/// field is a native subview the view places over `brief.field`; its ground
/// and border are drawn here too, so the card reads whole before it lands.
fn draw_overlay(
    sh: &Shared,
    m: &AppKitMetrics<'_>,
    brief: &BriefLayout,
    scrim: &Rect,
    hovered: ListHit,
    pressed: ListHit,
) {
    fill_round(scrim, 0.0, tokens::get().black, BRIEF_SCRIM);
    fill_round(&brief.card, BRIEF_RADIUS, tokens::get().tile, 1.0);
    stroke_round(&brief.card, BRIEF_RADIUS, 1.0, tokens::get().border_subtle, 1.0);

    // Header: the warn dot, "Brief for <from>", the age at the right.
    let header = &brief.header;
    fill_dot(header.x + 4.0, header.y + header.h / 2.0, STATE_DOT_D, tokens::get().awaiting, 1.0);
    let title = Rect::new(header.x + HEADER_TEXT_X, header.y, (header.w - HEADER_TEXT_X - 40.0).max(0.0), header.h);
    draw_text(sh, &format!("Brief for {}", brief.from), &title, Style::Title, tokens::get().text_primary, 1.0, Align::Left, false);
    let age_w = m.width(&brief.age, Style::Number);
    draw_text(
        sh,
        &brief.age,
        &Rect::new(header.right() - age_w, header.y, age_w, header.h),
        Style::Number,
        tokens::get().text_tertiary,
        1.0,
        Align::Right,
        false,
    );

    draw_wrapped(sh, &brief.message, &brief.message_rect, Style::Secondary, tokens::get().text_secondary, 1.0);

    // The options: a picked one takes the accent ground and white text.
    for chip in &brief.choices {
        let selected = brief.picked == Some(chip.index);
        let hit = AsksHit::BriefChip(brief.key, chip.index);
        let hot = matches!(hovered, ListHit::Ask(h) if h == hit);
        let down = matches!(pressed, ListHit::Ask(h) if h == hit);
        let radius = chip.rect.h / 2.0;
        if selected {
            fill_round(&chip.rect, radius, if down { tokens::get().accent_pressed } else { tokens::get().accent }, 1.0);
        } else {
            if hot || down {
                fill_round(&chip.rect, radius, tokens::get().white, if down { 0.12 } else { 0.08 });
            }
            stroke_round(&chip.rect, radius, 1.0, tokens::get().border_subtle, 1.0);
        }
        let fg = if selected { tokens::get().text_on_fill } else { tokens::get().text_secondary };
        draw_text(sh, &chip.label, &chip.rect, Style::Chip, fg, 1.0, Align::Center, false);
    }

    // The field's box. The native field covers it while the overlay is open.
    fill_round(&brief.field, 6.0, tokens::get().ground, 1.0);
    stroke_round(&brief.field, 6.0, 1.0, tokens::get().border_subtle, 1.0);

    // Cancel is a ghost; Send takes the accent, faded while it has nothing
    // to send.
    let cancel_hit = AsksHit::BriefCancel(brief.key);
    let cancel_down = matches!(pressed, ListHit::Ask(h) if h == cancel_hit);
    let cancel_hot = matches!(hovered, ListHit::Ask(h) if h == cancel_hit);
    if cancel_hot || cancel_down {
        fill_round(&brief.cancel, SNOOZE_RADIUS, tokens::get().white, if cancel_down { 0.12 } else { 0.08 });
    }
    draw_text(sh, "Cancel", &brief.cancel, Style::Chip, tokens::get().text_secondary, 1.0, Align::Center, false);
    let send_hit = AsksHit::BriefSend(brief.key);
    let send_down = matches!(pressed, ListHit::Ask(h) if h == send_hit);
    let send_ground = if send_down { tokens::get().accent_pressed } else { tokens::get().accent };
    fill_round(&brief.send, SNOOZE_RADIUS, send_ground, if brief.send_enabled { 1.0 } else { 0.4 });
    let send_alpha = if brief.send_enabled { 1.0 } else { 0.6 };
    let text_w = m.width("Send", Style::Chip);
    let total = BRIEF_SEND_ICON + BRIEF_SEND_ICON_GAP + text_w;
    let icon_x = brief.send.x + (brief.send.w - total) / 2.0;
    stroke_icon(
        super::feather::Icon::Send,
        &Rect::new(icon_x, brief.send.y + (brief.send.h - BRIEF_SEND_ICON) / 2.0, BRIEF_SEND_ICON, BRIEF_SEND_ICON),
        BRIEF_SEND_ICON,
        tokens::get().text_on_fill,
        send_alpha,
    );
    draw_text(
        sh,
        "Send",
        &Rect::new(icon_x + BRIEF_SEND_ICON + BRIEF_SEND_ICON_GAP, brief.send.y, text_w, brief.send.h),
        Style::Chip,
        tokens::get().text_on_fill,
        send_alpha,
        Align::Left,
        false,
    );

    draw_wrapped(
        sh,
        "Pick an option or not, the instruction goes with it. Cmd+Enter sends",
        &brief.hint,
        Style::Hint,
        tokens::get().text_tertiary,
        1.0,
    );
}

fn draw_header(sh: &Shared, m: &AppKitMetrics<'_>, card: &CardLayout, alpha: f64) {
    let header = card.header;
    fill_dot(header.x + 4.0, header.y + header.h / 2.0, STATE_DOT_D, tokens::get().awaiting, alpha);
    let age_w = m.width(&card.card.age, Style::Number);
    let age = Rect::new(header.right() - age_w, header.y, age_w, header.h);
    let text_x = header.x + HEADER_TEXT_X;
    let (from_w, tag, tab_w) = header_columns(&card.card, header, m);
    draw_text(
        sh,
        &card.card.from,
        &Rect::new(text_x, header.y, from_w, header.h),
        Style::Title,
        tokens::get().text_primary,
        alpha,
        Align::Left,
        false,
    );
    let tab_x = match tag {
        Some(tag) => {
            fill_round(&tag, TAG_RADIUS, tokens::get().white, 0.08 * alpha);
            stroke_round(&tag, TAG_RADIUS, 1.0, tokens::get().border_subtle, alpha);
            let text = tag_text(&card.card).unwrap_or(TAG_TEXT);
            draw_text(sh, text, &tag, Style::Hint, tokens::get().text_tertiary, alpha, Align::Center, false);
            tag.right() + HEADER_TAB_GAP
        }
        None => text_x + from_w + HEADER_TAB_GAP,
    };
    if tab_w > 0.0 {
        draw_text(
            sh,
            &card.card.tab,
            &Rect::new(tab_x, header.y, tab_w, header.h),
            Style::Summary,
            tokens::get().text_tertiary,
            alpha,
            Align::Left,
            false,
        );
    }
    let age_fg = if card.card.urgent { tokens::get().warn_text } else { tokens::get().text_tertiary };
    draw_text(sh, &card.card.age, &age, Style::Number, age_fg, alpha, Align::Right, false);
}

/// The command block: `bg0` ground, radius 4, 6 pt padding, one Menlo 11 line
/// per row. The wrapping came from `code_lines`, so a line fits its box.
fn draw_code(block: &Rect, lines: &[String], alpha: f64) {
    fill_round(block, CODE_RADIUS, tokens::get().ground, alpha);
    let font = mono_font();
    let attrs = mono_attrs(&font, tokens::get().text_secondary, alpha);
    for (i, line) in lines.iter().enumerate() {
        let row = Rect::new(
            block.x + CODE_PAD,
            block.y + CODE_PAD + i as f64 * CODE_LINE_H,
            (block.w - 2.0 * CODE_PAD).max(0.0),
            CODE_LINE_H,
        );
        unsafe {
            NSString::from_str(line).drawInRect_withAttributes(
                CGRect { origin: CGPoint { x: row.x, y: row.y }, size: CGSize { width: row.w, height: row.h } },
                Some(&attrs),
            )
        };
    }
}

/// Menlo 11, the one face the sidebar draws outside the design type scale.
fn mono_font() -> objc2::rc::Retained<NSFont> {
    NSFont::fontWithName_size(&NSString::from_str("Menlo"), MONO_SIZE).unwrap_or_else(|| {
        NSFont::systemFontOfSize_weight(MONO_SIZE, unsafe { objc2_app_kit::NSFontWeightRegular })
    })
}

fn mono_attrs(font: &NSFont, c: [f32; 3], alpha: f64) -> objc2::rc::Retained<NSDictionary<NSString, AnyObject>> {
    let color = NSColor::colorWithSRGBRed_green_blue_alpha(c[0] as f64, c[1] as f64, c[2] as f64, alpha);
    // One row per line: a command wider than the block is clipped, never
    // wrapped into the line below.
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setLineBreakMode(NSLineBreakMode::ByClipping);
    unsafe {
        NSDictionary::from_slices::<NSString>(
            &[
                objc2_app_kit::NSFontAttributeName,
                objc2_app_kit::NSForegroundColorAttributeName,
                objc2_app_kit::NSParagraphStyleAttributeName,
            ],
            &[font as &AnyObject, &*color as &AnyObject, &*paragraph as &AnyObject],
        )
    }
}

/// The note line's two options, `[check icon, its label, x icon, its label]`:
/// the second option starts 6 pt after the first label's text, so the pair
/// reads as one line rather than two columns. Labels clip at `right`.
fn note_options(note: Rect, right: f64, m: &dyn TextMetrics, options: &[String; 2]) -> [Rect; 4] {
    let first_x = note.x + NOTE_ICON + NOTE_ICON_GAP;
    let first_w = m.width(&options[0], Style::Hint);
    let cross_x = first_x + first_w + NOTE_OPTION_GAP;
    let second_x = cross_x + NOTE_ICON + NOTE_ICON_GAP;
    [
        Rect::new(note.x, note.y, NOTE_ICON, note.h),
        Rect::new(first_x, note.y, first_w, note.h),
        Rect::new(cross_x, note.y, NOTE_ICON, note.h),
        Rect::new(second_x, note.y, (right - second_x).max(0.0), note.h),
    ]
}

fn draw_options(sh: &Shared, m: &AppKitMetrics<'_>, card: &CardLayout, alpha: f64) {
    let note = &card.note;
    let mut options_right = note.right();
    if card.card.ended {
        let label = "Session ended";
        let text_w = m.width(label, Style::Hint);
        let width = NOTE_ICON + NOTE_ICON_GAP + text_w;
        let x = (note.right() - width).max(note.x);
        stroke_icon(
            super::feather::Icon::Info,
            &Rect::new(x, note.y, NOTE_ICON, note.h),
            NOTE_ICON,
            tokens::get().text_tertiary,
            alpha,
        );
        draw_text(
            sh,
            label,
            &Rect::new(x + NOTE_ICON + NOTE_ICON_GAP, note.y, text_w, note.h),
            Style::Hint,
            tokens::get().text_tertiary,
            alpha,
            Align::Left,
            false,
        );
        options_right = x - 8.0;
    }
    let [check, first, cross, second] = note_options(*note, options_right, m, &card.card.options);
    if card.card.decision_url.is_some() {
        // The one answer a decision has: Done, once the sheet is filled in.
        let first = Rect::new(first.x, first.y, (options_right - first.x).max(0.0), first.h);
        stroke_icon(super::feather::Icon::Check, &check, NOTE_ICON, tokens::get().ok_dot, alpha);
        let label = format!("{} once the sheet is filled in", card.card.options[0]);
        draw_text(sh, &label, &first, Style::Hint, tokens::get().text_tertiary, alpha, Align::Left, false);
        return;
    }
    stroke_icon(super::feather::Icon::Check, &check, NOTE_ICON, tokens::get().ok_dot, alpha);
    draw_text(
        sh,
        &card.card.options[0],
        &first,
        Style::Hint,
        tokens::get().text_tertiary,
        alpha,
        Align::Left,
        false,
    );
    stroke_icon(super::feather::Icon::X, &cross, NOTE_ICON, tokens::get().interrupt, alpha);
    draw_text(
        sh,
        &card.card.options[1],
        &second,
        Style::Hint,
        tokens::get().text_tertiary,
        alpha,
        Align::Left,
        false,
    );
}

fn draw_button(sh: &Shared, hit: AsksHit, rect: &Rect, alpha: f64, hovered: ListHit, pressed: ListHit) {
    let hot = matches!(hovered, ListHit::Ask(h) if h == hit);
    let down = matches!(pressed, ListHit::Ask(h) if h == hit);
    match hit {
        AsksHit::Ok(_)
        | AsksHit::Ko(_)
        | AsksHit::Brief(_)
        | AsksHit::PermissionApprove(_)
        | AsksHit::PermissionReject(_) => {
            let ground = if down || hot { tokens::get().tile_pressed } else { tokens::get().tile_hover };
            fill_round(rect, AB_RADIUS, ground, alpha);
            let (icon, fg) = match hit {
                AsksHit::Ok(_) | AsksHit::PermissionApprove(_) => (super::feather::Icon::Check, tokens::get().ok_dot),
                AsksHit::Ko(_) | AsksHit::PermissionReject(_) => (super::feather::Icon::X, tokens::get().interrupt),
                _ => (super::feather::Icon::Edit, tokens::get().text_primary),
            };
            stroke_icon(icon, rect, AB_ICON, fg, alpha);
        }
        AsksHit::Open(_) | AsksHit::SnoozeMenu(_) => {
            if hot || down {
                fill_round(rect, AB_RADIUS, tokens::get().white, if down { 0.16 } else { 0.10 });
            }
            let icon = if matches!(hit, AsksHit::Open(_)) {
                super::feather::Icon::Open
            } else {
                super::feather::Icon::Moon
            };
            let fg = if hot || down { tokens::get().text_primary } else { tokens::get().text_tertiary };
            stroke_icon(icon, rect, GHOST_ICON, fg, alpha);
        }
        AsksHit::Snooze(_, choice) => {
            let ground = if down || hot { tokens::get().tile_pressed } else { tokens::get().tile_hover };
            fill_round(rect, SNOOZE_RADIUS, ground, alpha);
            draw_text(sh, choice.label(), rect, Style::Chip, tokens::get().text_secondary, alpha, Align::Center, false);
        }
        AsksHit::PermissionAlways(_) => {
            let ground = if down || hot { tokens::get().tile_pressed } else { tokens::get().tile_hover };
            fill_round(rect, SNOOZE_RADIUS, ground, alpha);
            draw_text(sh, "Always", rect, Style::Chip, tokens::get().text_secondary, alpha, Align::Center, false);
        }
        AsksHit::OpenDoc(_) => {
            // The design system's primary button: what the card is for.
            let ground = if down || hot { tokens::get().accent_pressed } else { tokens::get().accent };
            fill_round(rect, AB_RADIUS, ground, alpha);
            draw_text(sh, OPEN_DOC_TEXT, rect, Style::Chip, tokens::get().text_on_fill, alpha, Align::Center, false);
        }
        AsksHit::ShowNow(_) => {
            let ground = if down || hot { tokens::get().tile_pressed } else { tokens::get().tile_hover };
            fill_round(rect, SNOOZE_RADIUS, ground, alpha);
            draw_text(sh, "Show now", rect, Style::Chip, tokens::get().text_primary, alpha, Align::Center, false);
        }
        // The overlay's chips and buttons are drawn whole by `draw_overlay`.
        AsksHit::BriefChip(_, _) | AsksHit::BriefSend(_) | AsksHit::BriefCancel(_) | AsksHit::ToggleSnoozed => {}
    }
}

fn draw_snoozed_header(
    sh: &Shared,
    m: &AppKitMetrics<'_>,
    rect: &Rect,
    count: usize,
    folded: bool,
    hovered: ListHit,
    pressed: ListHit,
) {
    let hit = AsksHit::ToggleSnoozed;
    let hot = matches!(hovered, ListHit::Ask(h) if h == hit);
    let down = matches!(pressed, ListHit::Ask(h) if h == hit);
    if hot || down {
        fill_round(rect, 6.0, tokens::get().white, if down { 0.16 } else { 0.08 });
    }
    let icon = if folded { super::feather::Icon::ChevronRight } else { super::feather::Icon::ChevronDown };
    let chevron = Rect::new(rect.x + 2.0, rect.y + (rect.h - SNOOZE_CHEVRON) / 2.0, SNOOZE_CHEVRON, SNOOZE_CHEVRON);
    stroke_icon(icon, &chevron, SNOOZE_CHEVRON, tokens::get().text_tertiary, 1.0);
    let label = format!("Snoozed {count}");
    let w = m.width(&label, Style::Chip);
    draw_text(
        sh,
        &label,
        &Rect::new(rect.x + SNOOZE_TEXT_X, rect.y, w.max(0.0), rect.h),
        Style::Chip,
        tokens::get().text_secondary,
        1.0,
        Align::Left,
        false,
    );
}

fn draw_empty(sh: &Shared, rect: &Rect) {
    let icon = Rect::new(rect.x + (rect.w - EMPTY_ICON) / 2.0, rect.y + 8.0, EMPTY_ICON, EMPTY_ICON);
    stroke_icon(super::feather::Icon::CheckCircle, &icon, EMPTY_ICON, tokens::get().text_tertiary, 0.7);
    draw_text(
        sh,
        "No open questions",
        &Rect::new(rect.x, icon.bottom() + 10.0, rect.w, 20.0),
        Style::Title,
        tokens::get().text_secondary,
        1.0,
        Align::Center,
        false,
    );
    draw_text(
        sh,
        "Agents ask here when they need you",
        &Rect::new(rect.x, icon.bottom() + 32.0, rect.w, 16.0),
        Style::Hint,
        tokens::get().text_tertiary,
        1.0,
        Align::Center,
        false,
    );
}

// ---------------------------------------------------------------
// Clock
// ---------------------------------------------------------------

/// Epoch milliseconds, for the transient and highlight deadlines.
pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn local_tm(secs: u64) -> libc::tm {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = secs as libc::time_t;
    unsafe { libc::localtime_r(&t, &mut tm) };
    tm
}

fn local_hhmm(secs: u64) -> String {
    let tm = local_tm(secs);
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

/// The epoch second of `hour:minute` on the local day `day_offset` days after
/// `now`. `mktime` normalizes a day past the end of the month.
fn local_epoch(now: u64, day_offset: i32, hour: i32, minute: i32) -> u64 {
    let mut tm = local_tm(now);
    tm.tm_hour = hour;
    tm.tm_min = minute;
    tm.tm_sec = 0;
    tm.tm_mday += day_offset;
    tm.tm_isdst = -1;
    let secs = unsafe { libc::mktime(&mut tm) };
    if secs < 0 { now + 3600 } else { secs as u64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Six points per character, one line per 40 points of width.
    struct FakeMetrics;

    impl TextMetrics for FakeMetrics {
        fn width(&self, text: &str, _style: Style) -> f64 {
            text.chars().count() as f64 * 6.0
        }

        fn lines(&self, text: &str, _style: Style, width: f64, max: usize) -> usize {
            ((text.chars().count() as f64 * 6.0 / width).ceil() as usize).clamp(1, max)
        }
    }

    const W: f64 = 280.0;
    const NOW_MS: u64 = 1_700_000_000_000;

    fn card(key: u64, id: &str, message: &str) -> AskCardVm {
        AskCardVm {
            key,
            id: id.into(),
            from: "claude".into(),
            tab: "kova".into(),
            pane_id: Some(3),
            age: "4m".into(),
            urgent: false,
            message: message.into(),
            options: ["Ship it".into(), "Wait".into()],
            choices: vec!["Ship it".into(), "Wait".into()],
            created_at: 100,
            ended: false,
            snoozed: false,
            back_at: String::new(),
            permission: None,
            decision_url: None,
            step: None,
        }
    }

    fn state(cards: Vec<AskCardVm>) -> AsksState {
        AsksState { cards, ..Default::default() }
    }

    fn message_of(lines: usize) -> String {
        // 236 pt of inner width, six points a character: 39 fit on one line.
        "x".repeat(lines * 40 - 10)
    }

    #[test]
    fn card_heights_grow_with_the_message_and_stop_at_six_lines() {
        let one = layout(&state(vec![card(1, "a", &message_of(1))]), W, &FakeMetrics);
        let three = layout(&state(vec![card(1, "a", &message_of(3))]), W, &FakeMetrics);
        let eight = layout(&state(vec![card(1, "a", &message_of(8))]), W, &FakeMetrics);
        assert_eq!(one.cards[0].message_lines, 1);
        assert_eq!(three.cards[0].message_lines, 3);
        // Eight lines are cut at six: taller than three, the same as six.
        assert_eq!(eight.cards[0].message_lines, MSG_MAX_LINES);
        let six = layout(&state(vec![card(1, "a", &message_of(6))]), W, &FakeMetrics);
        assert_eq!(six.cards[0].message_lines, 6);
        assert_eq!(one.cards[0].frame.h, 115.0);
        assert_eq!(three.cards[0].frame.h, 145.0);
        assert_eq!(eight.cards[0].frame.h, 190.0);
        assert_eq!(eight.cards[0].frame.h, six.cards[0].frame.h);
    }

    #[test]
    fn a_decision_card_opens_its_sheet_and_answers_done_without_ko() {
        let mut c = card(7, "a", "3 decisions on the side panel");
        c.decision_url = Some("https://claude.ai/artifact/x".into());
        c.options = ["Done".into(), "No".into()];
        let mut st = state(vec![c]);
        let l = layout(&st, W, &FakeMetrics);
        let card = &l.cards[0];
        assert!(card.tag.is_some(), "the Decision tag");
        let hits: Vec<AsksHit> = card.buttons.iter().map(|(hit, _)| *hit).collect();
        assert!(hits.contains(&AsksHit::OpenDoc(7)) && hits.contains(&AsksHit::Ok(7)));
        assert!(!hits.contains(&AsksHit::Ko(7)));
        let rect_of = |want: AsksHit| card.buttons.iter().find(|(hit, _)| *hit == want).map(|(_, r)| *r).expect("a button");
        assert_eq!(rect_of(AsksHit::OpenDoc(7)).x, card.frame.x + CARD_PAD_H);
        assert_eq!(rect_of(AsksHit::Ok(7)).x, rect_of(AsksHit::OpenDoc(7)).right() + AB_GAP);
        for (hit, rect) in &card.buttons {
            assert_eq!(l.hit(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0), Some(*hit));
        }
        // Open answers nothing; OK answers Done.
        assert_eq!(click(&mut st, AsksHit::OpenDoc(7), NOW_MS), None);
        assert!(st.transient.is_none());
        assert_eq!(
            click(&mut st, AsksHit::Ok(7), NOW_MS),
            Some(AskAction::Answer { id: "a".into(), action: "Done".into(), text: String::new() })
        );
    }

    #[test]
    fn a_decision_question_carries_its_url_into_the_card() {
        let mut q = crate::ask::Question {
            id: "6f1c0000-0000-4000-8000-000000000000".into(),
            message: "Pick".into(),
            actions: vec!["Done".into()],
            input: false,
            from: crate::ask::Asker::from_env(|_| None, Some("pm".into()), "/tmp"),
            asked_at: 1,
            snoozed_until: 0,
            kind: Some(crate::ask::KIND_DECISION.into()),
            url: Some("https://claude.ai/artifact/x".into()),
            run: None,
            sheet: None,
            draft: None,
            follow_up: false,
        };
        let vm = AskCardVm::from_question(&q, None, false, 10);
        assert_eq!(vm.decision_url.as_deref(), Some("https://claude.ai/artifact/x"));
        assert_eq!(vm.options[0], "Done");
        // A url without the decision kind is not a decision.
        q.kind = None;
        assert_eq!(AskCardVm::from_question(&q, None, false, 10).decision_url, None);
    }

    #[test]
    fn every_button_sits_inside_its_card_and_every_button_hits() {
        let l = layout(&state(vec![card(7, "a", "short message")]), W, &FakeMetrics);
        let card = &l.cards[0];
        assert_eq!(card.buttons.len(), 5);
        for (hit, rect) in &card.buttons {
            assert!(card.frame.contains(rect.x, rect.y), "{hit:?} starts outside the card");
            assert!(card.frame.contains(rect.right() - 0.5, rect.bottom() - 0.5), "{hit:?} ends outside");
            assert_eq!(l.hit(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0), Some(*hit));
        }
        // The three action buttons are left to right, then the ghosts at the right.
        let rect_of = |want: AsksHit| card.buttons.iter().find(|(hit, _)| *hit == want).map(|(_, r)| *r).expect("a button");
        assert_eq!(rect_of(AsksHit::Ok(7)).x, card.frame.x + CARD_PAD_H);
        assert_eq!(rect_of(AsksHit::Ko(7)).x, rect_of(AsksHit::Ok(7)).right() + AB_GAP);
        assert_eq!(rect_of(AsksHit::Brief(7)).x, rect_of(AsksHit::Ko(7)).right() + AB_GAP);
        assert_eq!(rect_of(AsksHit::SnoozeMenu(7)).right(), card.frame.right() - CARD_PAD_H);
        assert_eq!(rect_of(AsksHit::Open(3)).right(), rect_of(AsksHit::SnoozeMenu(7)).x - GHOST_GAP);
        // Pressing the card's body is not a target.
        assert_eq!(l.hit(card.frame.x + 2.0, card.frame.y + 2.0), None);
    }

    #[test]
    fn a_card_without_a_pane_has_no_open_button() {
        let mut c = card(7, "a", "short");
        c.pane_id = None;
        let l = layout(&state(vec![c]), W, &FakeMetrics);
        assert!(!l.cards[0].buttons.iter().any(|(hit, _)| matches!(hit, AsksHit::Open(_))));
    }

    #[test]
    fn the_snooze_row_toggles_and_offers_three_choices() {
        let mut st = state(vec![card(7, "a", "short")]);
        let closed = layout(&st, W, &FakeMetrics);
        let closed_h = closed.cards[0].frame.h;
        assert!(click(&mut st, AsksHit::SnoozeMenu(7), NOW_MS).is_none());
        assert_eq!(st.snooze_open.as_deref(), Some("a"));

        let open = layout(&st, W, &FakeMetrics);
        let row = open.cards[0].snooze_row.expect("the row is open");
        assert!(open.cards[0].frame.h > closed_h);
        for (i, choice) in SnoozeChoice::ALL.iter().enumerate() {
            assert_eq!(
                open.hit(row[i].x + row[i].w / 2.0, row[i].y + row[i].h / 2.0),
                Some(AsksHit::Snooze(7, *choice))
            );
            assert!(row[i].x + row[i].w <= open.cards[0].frame.right());
        }
        // Every choice lands in the future, inside the board's 24 h bound.
        for choice in SnoozeChoice::ALL {
            let until = choice.until(NOW_MS / 1000);
            assert!(until > NOW_MS / 1000);
            assert!(until <= NOW_MS / 1000 + crate::ask::SNOOZE_MAX_SECS);
        }
        let action = click(&mut st, AsksHit::Snooze(7, SnoozeChoice::OneHour), NOW_MS).expect("a snooze");
        assert_eq!(action, AskAction::Snooze { id: "a".into(), until: NOW_MS / 1000 + 3600 });
        assert_eq!(st.snooze_open, None);

        // The moon toggles it back shut.
        assert!(click(&mut st, AsksHit::SnoozeMenu(7), NOW_MS).is_none());
        assert!(st.snooze_open.is_some());
        assert!(click(&mut st, AsksHit::SnoozeMenu(7), NOW_MS).is_none());
        assert_eq!(st.snooze_open, None);
    }

    #[test]
    fn a_double_click_answers_once_and_the_queue_drains_once() {
        let mut st = state(vec![card(7, "a", "short")]);
        let mut queue = AskQueue::default();
        if let Some(action) = click(&mut st, AsksHit::Ok(7), NOW_MS) {
            queue.push(action);
        }
        assert_eq!(queue.len(), 1);
        assert!(click(&mut st, AsksHit::Ok(7), NOW_MS + 16).is_none());
        let sent = st.transient.as_ref().expect("the card is sending");
        assert_eq!(sent.phase, TransientPhase::Sending);
        assert_eq!(queue.drain().len(), 1);
        assert_eq!(queue.drain().len(), 0);
    }

    #[test]
    fn a_handoff_step_leaves_the_card_open_and_fait_sends_it() {
        let mut handoff = card(7, "a", "Log in");
        handoff.options = ["Ouvrir".into(), "Fait".into()];
        handoff.step = Some("Ouvrir".into());
        let mut st = state(vec![handoff]);
        let step = click(&mut st, AsksHit::Ok(7), NOW_MS);
        assert_eq!(step, Some(AskAction::Answer { id: "a".into(), action: "Ouvrir".into(), text: String::new() }));
        assert!(st.transient.is_none(), "nothing is sent: the question stays");
        let done = click(&mut st, AsksHit::Ko(7), NOW_MS + 16);
        assert_eq!(done, Some(AskAction::Answer { id: "a".into(), action: "Fait".into(), text: String::new() }));
        assert_eq!(st.transient.as_ref().map(|t| t.phase), Some(TransientPhase::Sending));
    }

    #[test]
    fn a_handoff_card_shows_its_url_and_keeps_the_step_off_the_brief() {
        let mut q = question(vec!["Ouvrir", "Fait"], 1);
        q.kind = Some(crate::handoff::KIND.into());
        q.url = Some("https://x.y/login".into());
        let vm = AskCardVm::from_question(&q, None, false, 10);
        assert_eq!(vm.options, ["Ouvrir".to_string(), "Fait".to_string()]);
        assert_eq!(vm.choices, ["Fait"]);
        assert_eq!(vm.step.as_deref(), Some("Ouvrir"));
        assert_eq!(vm.message, "Ship it?\n\nhttps://x.y/login");
    }

    #[test]
    fn ok_and_ko_send_the_two_options() {
        let mut st = state(vec![card(7, "a", "short")]);
        let action = click(&mut st, AsksHit::Ok(7), NOW_MS).expect("an answer");
        assert_eq!(action, AskAction::Answer { id: "a".into(), action: "Ship it".into(), text: String::new() });

        let mut st = state(vec![card(7, "a", "short")]);
        let action = click(&mut st, AsksHit::Ko(7), NOW_MS).expect("an answer");
        assert_eq!(action, AskAction::Answer { id: "a".into(), action: "Wait".into(), text: String::new() });

        // A single-option question answers `No` on the cross.
        let mut single = card(7, "a", "short");
        single.options = ["Only".into(), "No".into()];
        let mut st = state(vec![single]);
        let action = click(&mut st, AsksHit::Ko(7), NOW_MS).expect("a No");
        assert_eq!(action, AskAction::Answer { id: "a".into(), action: "No".into(), text: String::new() });
    }

    #[test]
    fn the_empty_state_says_what_the_view_is_for() {
        let l = layout(&AsksState::default(), W, &FakeMetrics);
        assert!(l.shown);
        assert!(l.cards.is_empty());
        assert!(l.empty.is_some());
        assert_eq!(l.content_h, LIST_TOP + EMPTY_H + LIST_BOTTOM);
        assert_eq!(l.hit(PAD_H + 10.0, LIST_TOP + 10.0), None);
    }

    #[test]
    fn the_snoozed_section_folds_by_default_and_show_now_opens_it() {
        let mut snoozed = card(7, "a", "later");
        snoozed.snoozed = true;
        snoozed.back_at = "20:00".into();
        let mut st = state(vec![snoozed]);

        let folded = layout(&st, W, &FakeMetrics);
        assert_eq!(folded.snoozed_count, 1);
        assert!(folded.snoozed_header.is_some());
        assert!(folded.snoozed_cards.is_empty());
        let header = folded.snoozed_header.expect("a header");
        assert_eq!(folded.hit(header.x + 4.0, header.y + 4.0), Some(AsksHit::ToggleSnoozed));

        assert!(click(&mut st, AsksHit::ToggleSnoozed, NOW_MS).is_none());
        assert!(!st.folded);
        let open = layout(&st, W, &FakeMetrics);
        assert_eq!(open.snoozed_cards.len(), 1);
        assert!(open.content_h > folded.content_h);
        let card = &open.snoozed_cards[0];
        assert!(card.faded);
        let show = card.buttons[0].1;
        assert_eq!(open.hit(show.x + show.w / 2.0, show.y + show.h / 2.0), Some(AsksHit::ShowNow(7)));
        let action = click(&mut st, AsksHit::ShowNow(7), NOW_MS).expect("an unsnooze");
        assert_eq!(action, AskAction::Unsnooze { id: "a".into() });
    }

    #[test]
    fn only_the_highlighted_card_is_lit() {
        let st = AsksState {
            cards: vec![card(1, "a", "first"), card(2, "b", "second")],
            highlight: Some(Highlight { id: "b".into(), until_ms: NOW_MS + HIGHLIGHT_MS }),
            ..Default::default()
        };
        let l = layout(&st, W, &FakeMetrics);
        assert!(!l.cards.iter().find(|c| c.card.id == "a").expect("a").highlighted);
        assert!(l.cards.iter().find(|c| c.card.id == "b").expect("b").highlighted);
        // Newest first: the two cards share a created_at, so the id breaks the tie.
        assert_eq!(l.cards[0].card.id, "a");
        assert_eq!(l.cards[1].card.id, "b");
    }

    #[test]
    fn a_sending_card_becomes_sent_then_disappears() {
        let mut st = state(vec![card(7, "a", "short")]);
        click(&mut st, AsksHit::Ok(7), NOW_MS);
        // The board still holds it: sending stays.
        st.prune(&[7], NOW_MS + 10);
        assert_eq!(st.transient.as_ref().map(|t| t.phase), Some(TransientPhase::Sending));
        // The board let it go: sent for 1.5 s.
        st.prune(&[], NOW_MS + 20);
        let sent = st.transient.as_ref().expect("sent");
        assert_eq!(sent.phase, TransientPhase::Sent);
        assert_eq!(sent.until_ms, NOW_MS + 20 + SENT_LINGER_MS);
        // The old card is still in the layout under the transient.
        let l = layout(&st, W, &FakeMetrics);
        assert_eq!(l.cards[0].phase, Some(TransientPhase::Sent));
        st.prune(&[], NOW_MS + 20 + SENT_LINGER_MS + 1);
        assert!(st.transient.is_none());
    }

    #[test]
    fn a_sending_click_gives_up_when_the_board_never_takes_it() {
        let mut st = state(vec![card(7, "a", "short")]);
        click(&mut st, AsksHit::Ok(7), NOW_MS);
        st.prune(&[7], NOW_MS + SENDING_TIMEOUT_MS + 1);
        assert!(st.transient.is_none());
        // And the card can be answered again.
        assert!(click(&mut st, AsksHit::Ok(7), NOW_MS + SENDING_TIMEOUT_MS + 2).is_some());
    }

    #[test]
    fn a_highlight_expires_after_two_seconds() {
        let mut st = state(vec![card(7, "a", "short")]);
        st.highlight = Some(Highlight { id: "a".into(), until_ms: NOW_MS + HIGHLIGHT_MS });
        st.prune(&[7], NOW_MS + HIGHLIGHT_MS - 1);
        assert!(st.highlight.is_some());
        st.prune(&[7], NOW_MS + HIGHLIGHT_MS);
        assert!(st.highlight.is_none());
    }

    fn question(actions: Vec<&str>, asked_at: u64) -> crate::ask::Question {
        crate::ask::Question {
            id: "11111111-2222-3333-4444-555555555555".into(),
            message: "Ship it?".into(),
            actions: actions.into_iter().map(str::to_string).collect(),
            input: false,
            from: crate::ask::Asker { name: "cli".into(), ..Default::default() },
            asked_at,
            snoozed_until: 0,
            kind: None,
            url: None,
            run: None,
            sheet: None,
            draft: None,
            follow_up: false,
        }
    }

    #[test]
    fn the_card_takes_the_question_the_pane_and_the_clock() {
        let now = 1_700_000_000;
        let q = question(vec!["Ship it", "Wait"], now - 240);
        let vm = AskCardVm::from_question(&q, Some((3, "kova".into())), false, now);
        assert_eq!(vm.age, "4m");
        assert!(!vm.urgent);
        assert_eq!(vm.tab, "kova");
        assert_eq!(vm.pane_id, Some(3));
        assert!(!vm.ended);
        assert_eq!(vm.options, ["Ship it".to_string(), "Wait".to_string()]);
        assert_eq!(vm.choices, ["Ship it".to_string(), "Wait".to_string()]);

        let old = AskCardVm::from_question(&question(vec!["Go"], now - 7200), None, false, now);
        assert!(old.urgent);
        assert!(!old.snoozed);
        // One action: the cross falls back to No.
        assert_eq!(old.options, ["Go".to_string(), "No".to_string()]);
        // No pane and no live conversation: the session is over.
        assert!(old.ended);
        // A live conversation elsewhere is not over, even without a pane.
        let live = AskCardVm::from_question(&q, None, true, now);
        assert!(!live.ended);
    }

    // ---------------------------------------------------------------
    // Brief overlay
    // ---------------------------------------------------------------

    fn brief_card() -> AskCardVm {
        let mut card = card(7, "a", "short message");
        card.choices = vec!["Ship it".into(), "Wait".into()];
        card
    }

    fn brief_state(cards: Vec<AskCardVm>, picked: Option<usize>, draft: &str) -> AsksState {
        let mut st = state(cards);
        let id = st.cards[0].id.clone();
        st.brief = Some(BriefOpen { id: id.clone(), picked });
        if !draft.is_empty() {
            st.drafts.insert(id, draft.to_string());
        }
        st
    }

    #[test]
    fn the_overlay_sits_below_the_top_and_wraps_its_chips() {
        let mut card = brief_card();
        card.choices = vec!["a".repeat(15), "b".repeat(15), "c".repeat(15)];
        let st = brief_state(vec![card], None, "");
        let narrow = layout(&st, 300.0, &FakeMetrics);
        let wide = layout(&st, 420.0, &FakeMetrics);
        let n = narrow.brief.expect("the overlay");
        let w = wide.brief.expect("the overlay");
        // 12 pt from each side and from the top of the list, 12 pt of padding.
        assert_eq!(n.card.x, 12.0);
        assert_eq!(n.card.y, LIST_TOP + 12.0);
        assert_eq!(n.card.w, 276.0);
        assert_eq!(w.card.w, 396.0);
        // One line of question, then the chips: 15 characters are 90 pt wide,
        // plus 9 pt of padding either side.
        assert_eq!(n.choices[0].rect, Rect::new(24.0, 79.0, 108.0, 22.0));
        assert_eq!(n.choices[1].rect.y, 79.0);
        assert_eq!(n.choices[2].rect.y, 107.0, "the third chip wraps at 300");
        assert_eq!(w.choices[2].rect.y, 79.0, "it fits one line at 420");
        // The field is three lines at least, whatever the draft.
        assert_eq!(n.field, Rect::new(24.0, 139.0, 252.0, 60.0));
        assert_eq!(w.field, Rect::new(24.0, 111.0, 372.0, 60.0));
        // Buttons right-aligned under the field, 8 pt apart.
        assert_eq!(n.cancel, Rect::new(144.0, 209.0, 60.0, 26.0));
        assert_eq!(n.send, Rect::new(212.0, 209.0, 64.0, 26.0));
        assert_eq!(w.cancel, Rect::new(264.0, 181.0, 60.0, 26.0));
        assert_eq!(w.send, Rect::new(332.0, 181.0, 64.0, 26.0));
        assert_eq!(n.card.bottom(), 281.0);
        assert_eq!(w.card.bottom(), 253.0);
        assert!(!n.send_enabled);
    }

    #[test]
    fn the_field_grows_from_three_lines_to_six() {
        // 252 pt of field, 8 pt either side: 39 characters a line.
        let one = brief_state(vec![brief_card()], None, &"x".repeat(10));
        let four = brief_state(vec![brief_card()], None, &"x".repeat(39 * 4));
        let ten = brief_state(vec![brief_card()], None, &"x".repeat(39 * 10));
        let height = |st: AsksState| layout(&st, 300.0, &FakeMetrics).brief.expect("open").field.h;
        assert_eq!(height(one), 60.0);
        assert_eq!(height(four), 80.0);
        assert_eq!(height(ten), 120.0, "six lines, then the field scrolls");
    }

    #[test]
    fn the_overlay_hits_chips_buttons_and_cancels_outside() {
        let st = brief_state(vec![brief_card()], Some(0), "an instruction");
        let l = layout(&st, 300.0, &FakeMetrics);
        let b = l.brief.as_ref().expect("the overlay");
        let centre = |r: Rect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let (x, y) = centre(b.choices[1].rect);
        assert_eq!(l.hit(x, y), Some(AsksHit::BriefChip(7, 1)));
        let (x, y) = centre(b.cancel);
        assert_eq!(l.hit(x, y), Some(AsksHit::BriefCancel(7)));
        let (x, y) = centre(b.send);
        assert_eq!(l.hit(x, y), Some(AsksHit::BriefSend(7)));
        // Outside the card is a cancel, including over the list behind.
        assert_eq!(l.hit(b.card.x - 1.0, b.card.y), Some(AsksHit::BriefCancel(7)));
        assert_eq!(l.hit(b.card.x + 10.0, b.card.bottom() + 4.0), Some(AsksHit::BriefCancel(7)));
        // Inside the card, on its text: nothing.
        assert_eq!(l.hit(b.header.x + 80.0, b.header.y + 4.0), None);
        assert_eq!(b.picked, Some(0));
        assert!(b.send_enabled);
    }

    #[test]
    fn a_chip_toggles_and_send_takes_the_picked_option() {
        let mut st = state(vec![brief_card()]);
        assert!(click(&mut st, AsksHit::Brief(7), NOW_MS).is_none());
        assert_eq!(st.brief.as_ref().and_then(|b| b.picked), None);
        assert!(click(&mut st, AsksHit::BriefChip(7, 1), NOW_MS).is_none());
        assert_eq!(st.brief.as_ref().and_then(|b| b.picked), Some(1));
        assert!(click(&mut st, AsksHit::BriefChip(7, 1), NOW_MS).is_none());
        assert_eq!(st.brief.as_ref().and_then(|b| b.picked), None, "a second click unpicks");
        click(&mut st, AsksHit::BriefChip(7, 0), NOW_MS);
        st.drafts.insert("a".into(), "  ship it  ".into());
        let action = click(&mut st, AsksHit::BriefSend(7), NOW_MS).expect("an answer");
        assert_eq!(
            action,
            AskAction::Answer { id: "a".into(), action: "Ship it".into(), text: "ship it".into() }
        );
        assert!(st.brief.is_none());
        assert_eq!(st.transient.as_ref().map(|t| t.phase), Some(TransientPhase::Sending));
    }

    #[test]
    fn with_no_chip_the_text_is_the_free_answer() {
        let mut st = state(vec![brief_card()]);
        click(&mut st, AsksHit::Brief(7), NOW_MS);
        assert!(click(&mut st, AsksHit::BriefSend(7), NOW_MS).is_none(), "nothing to send");
        assert!(st.brief.is_some(), "the overlay stays open");
        st.drafts.insert("a".into(), " use the cheap one ".into());
        let action = click(&mut st, AsksHit::BriefSend(7), NOW_MS).expect("an answer");
        assert_eq!(
            action,
            AskAction::Answer {
                id: "a".into(),
                action: crate::ask::OTHER.into(),
                text: "use the cheap one".into()
            }
        );
    }

    #[test]
    fn the_brief_text_is_capped_at_eight_kib_on_a_character_boundary() {
        let card = brief_card();
        let plain = brief_action(&card, None, &"x".repeat(crate::ask::ANSWER_TEXT_MAX + 100)).expect("an answer");
        let AskAction::Answer { text, .. } = plain else { panic!("an answer") };
        assert_eq!(text.len(), crate::ask::ANSWER_TEXT_MAX);
        // A two-byte character straddling the limit: the cut must land before
        // it rather than inside it.
        let cut = format!("{}{}", "x".repeat(crate::ask::ANSWER_TEXT_MAX - 2), "\u{e9}".repeat(2));
        let action = brief_action(&card, None, &cut).expect("an answer");
        let AskAction::Answer { text, .. } = action else { panic!("an answer") };
        assert_eq!(text.len(), crate::ask::ANSWER_TEXT_MAX);
        assert!(text.ends_with('\u{e9}'));
    }

    #[test]
    fn cancel_keeps_the_draft_and_a_vanished_ask_closes_the_overlay() {
        let mut st = state(vec![brief_card()]);
        click(&mut st, AsksHit::Brief(7), NOW_MS);
        st.drafts.insert("a".into(), "half typed".into());
        assert!(click(&mut st, AsksHit::BriefCancel(7), NOW_MS).is_none());
        assert!(st.brief.is_none());
        assert_eq!(st.drafts.get("a").map(String::as_str), Some("half typed"));

        // Reopening keeps the text; the board answering it elsewhere closes
        // the overlay and the draft stays for the record.
        click(&mut st, AsksHit::Brief(7), NOW_MS);
        assert!(st.brief.is_some());
        st.set_cards(Vec::new());
        assert!(st.brief.is_none());
        assert_eq!(st.drafts.get("a").map(String::as_str), Some("half typed"));
    }

    #[test]
    fn the_ok_and_ko_options_sit_together_on_the_note_line() {
        let note = Rect::new(10.0, 0.0, 200.0, NOTE_H);
        let options = ["Yes".to_string(), "No".to_string()];
        let [check, first, cross, second] = note_options(note, note.right(), &FakeMetrics, &options);
        assert_eq!(check, Rect::new(10.0, 0.0, NOTE_ICON, NOTE_H));
        assert_eq!(first.x, check.right() + NOTE_ICON_GAP);
        assert_eq!(first.w, 3.0 * 6.0);
        // Six points after the first label, not at the note's middle.
        assert_eq!(cross.x, first.right() + NOTE_OPTION_GAP);
        assert_eq!(cross.x, 10.0 + NOTE_ICON + NOTE_ICON_GAP + 18.0 + 6.0);
        assert_eq!(second.x, cross.right() + NOTE_ICON_GAP);
        assert_eq!(second.w, note.right() - second.x);
    }

    // ---------------------------------------------------------------
    // Permission cards
    // ---------------------------------------------------------------

    fn prompt(command: &str) -> ParsedPrompt {
        ParsedPrompt {
            header: "Bash command".into(),
            question: "Do you want to proceed?".into(),
            detail: Some(command.into()),
            lines: vec!["Bash command".into(), command.into()],
            options: vec![
                (1, "Yes".into()),
                (2, "Yes, and don't ask again for this command".into()),
                (3, "No, and tell Claude what to do differently".into()),
            ],
        }
    }

    fn permission(command: &str) -> AskCardVm {
        AskCardVm::from_permission(prompt(command), (3, "kova".into()), "claude".into(), 100, 100)
    }

    #[test]
    fn a_permission_card_carries_its_tag_its_code_block_and_the_three_answers() {
        let card = permission("cargo test");
        let key = card.key;
        assert_eq!(card.message, "Do you want to proceed?");
        assert_eq!(card.from, "claude");
        let permission = card.permission.as_ref().expect("a permission card");
        assert_eq!((permission.approve, permission.always, permission.reject), (Some(1), Some(2), Some(3)));
        assert!(card.id.starts_with("perm:3:"));

        let l = layout(&state(vec![card]), W, &FakeMetrics);
        let row = &l.cards[0];
        let tag = row.tag.expect("the Permission tag");
        assert!(row.header.contains(tag.x, tag.y), "the tag sits in the header");
        assert!(tag.w > 0.0 && tag.h == TAG_H);

        let code = row.code.expect("the command block");
        assert_eq!(row.code_lines, ["cargo test"], "one command, one line");
        assert_eq!(code.x, row.frame.x + CARD_PAD_H);
        assert_eq!(code.y, row.message.bottom() + CODE_GAP);
        assert_eq!(code.h, CODE_LINE_H + 2.0 * CODE_PAD);

        // Approve, Reject, Always, then the ghost that opens the pane. No OK,
        // Brief or Snooze: a permission has nothing else to say.
        let hits: Vec<AsksHit> = row.buttons.iter().map(|(hit, _)| *hit).collect();
        assert_eq!(
            hits,
            [
                AsksHit::PermissionApprove(key),
                AsksHit::PermissionReject(key),
                AsksHit::PermissionAlways(key),
                AsksHit::Open(3)
            ]
        );
        // Each button sits in the card, and a click on it lands there.
        for (hit, rect) in &row.buttons {
            assert!(row.frame.contains(rect.x, rect.y), "{hit:?} starts outside");
            assert_eq!(l.hit(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0), Some(*hit));
        }
        let approve = row.buttons[0].1;
        let reject = row.buttons[1].1;
        let always = row.buttons[2].1;
        assert_eq!(approve.h, AB_H);
        assert_eq!(reject.x, approve.right() + AB_GAP);
        assert_eq!(always.x, reject.right() + AB_GAP);
        assert_eq!(always.w, 6.0 * 6.0 + 2.0 * ALWAYS_PAD, "`Always` and its padding");
        assert_eq!(row.buttons[3].1.right(), row.frame.right() - CARD_PAD_H);
        // The block, the actions: no note line under them.
        assert_eq!(row.note, Rect::default());
        assert_eq!(row.frame.h, CODE_PAD * 2.0 + CODE_LINE_H + CODE_GAP + ACT_GAP + AB_H + CARD_PAD_V + CARD_PAD_V + HEADER_H + HEADER_GAP + MSG_LINE_H);
    }

    #[test]
    fn a_permission_without_parseable_options_only_opens_its_pane() {
        let mut card = permission("npm run build");
        let permission = card.permission.as_mut().expect("a permission");
        permission.prompt.options.clear();
        permission.approve = None;
        permission.always = None;
        permission.reject = None;
        let key = card.key;
        let l = layout(&state(vec![card]), W, &FakeMetrics);
        let hits: Vec<AsksHit> = l.cards[0].buttons.iter().map(|(hit, _)| *hit).collect();
        assert_eq!(hits, [AsksHit::Open(3)]);
        assert!(!hits.iter().any(|hit| hit.permission().is_some_and(|(k, _)| k == key)));
    }

    #[test]
    fn the_command_block_wraps_by_character_and_stops_at_four_lines() {
        // 236 pt wide, 12 pt of padding: 33 characters of Menlo 11 a line.
        let one = code_lines("cargo test", 236.0);
        assert_eq!(one, ["cargo test"]);
        let long = code_lines(&"x".repeat(40), 236.0);
        assert_eq!(long.len(), 2);
        assert_eq!(long[0].chars().count(), 33);
        assert_eq!(long[1].chars().count(), 7);
        // Five lines' worth: four rows, the last cut with an ellipsis.
        let many = code_lines(&"y".repeat(200), 236.0);
        assert_eq!(many.len(), CODE_MAX_LINES);
        assert!(many[3].ends_with('\u{2026}'));
        assert!(many[3].chars().count() <= 33);
    }

    #[test]
    fn a_permission_card_key_follows_the_screen_it_shows() {
        let a = permission("cargo test");
        let same = permission("cargo test");
        let other = permission("cargo build --release");
        assert_eq!(a.key, same.key, "the same screen keeps the card");
        assert_ne!(a.key, other.key, "another command is another card");
    }

    #[test]
    fn answering_a_permission_goes_sending_then_leaves_with_the_prompt() {
        let mut st = state(vec![permission("cargo test")]);
        let key = st.cards[0].key;
        permission_sending(&mut st, key, NOW_MS);
        let sending = st.transient.as_ref().expect("sending");
        assert_eq!(sending.phase, TransientPhase::Sending);
        assert_eq!(sending.until_ms, NOW_MS + SENDING_TIMEOUT_MS);
        // A second click lands on the same card: nothing more is sent.
        permission_sending(&mut st, key, NOW_MS + 10);
        assert_eq!(st.transient.as_ref().map(|t| t.until_ms), Some(NOW_MS + SENDING_TIMEOUT_MS));

        // The prompt left the screen: the card says Sent, then disappears.
        st.prune(&[], NOW_MS + 20);
        assert_eq!(st.transient.as_ref().map(|t| t.phase), Some(TransientPhase::Sent));
        st.prune(&[], NOW_MS + 20 + SENT_LINGER_MS + 1);
        assert!(st.transient.is_none());
    }

    #[test]
    fn a_refused_permission_click_says_why_in_crit_for_five_seconds() {
        let mut st = state(vec![permission("cargo test")]);
        permission_refused(&mut st, 3, NOW_MS);
        let l = layout(&st, W, &FakeMetrics);
        let flash = l.cards[0].flash.expect("the crit line");
        assert_eq!(flash.x, l.cards[0].frame.x + CARD_PAD_H);
        assert!(flash.bottom() <= l.cards[0].frame.bottom(), "the line stays in the card");
        // It holds for five seconds, then goes.
        st.prune(&[], NOW_MS + FLASH_MS - 1);
        assert!(st.flash.is_some());
        st.prune(&[], NOW_MS + FLASH_MS);
        assert!(st.flash.is_none());
        // Another pane's card never shows it.
        let mut other = permission("cargo test");
        other.pane_id = Some(4);
        let mut st = state(vec![other]);
        permission_refused(&mut st, 3, NOW_MS);
        assert!(layout(&st, W, &FakeMetrics).cards[0].flash.is_none());
    }

    #[test]
    fn the_permission_card_ignores_the_brief_and_snooze_hits() {
        let mut st = state(vec![permission("cargo test")]);
        let key = st.cards[0].key;
        assert!(click(&mut st, AsksHit::PermissionApprove(key), NOW_MS).is_none());
        assert!(st.transient.is_none(), "click alone sends nothing: the pane decides");
        assert!(st.brief.is_none());
        assert_eq!(st.snooze_open, None);
    }
}
