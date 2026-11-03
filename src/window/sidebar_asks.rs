//! The Asks view of the sidebar: the open `kova ask` questions as cards, with
//! the two answers the user actually needs (OK and KO), Brief, Snooze and the
//! pane the question came from. Pure: `layout` turns `AsksState` into rects,
//! `hit` turns a point into an `AsksHit`, `click` runs the state machine and
//! returns the action to queue on the delegate. `sidebar_ui` builds the cards
//! from `ask::Board`, `sidebar_view` draws them and dispatches the clicks.
//!
//! The card is Robin's 3 Nov design: `bg1` ground, 1 pt `b3` border, radius 8,
//! 10/12 padding, 8 pt apart. A highlighted card takes the selected rail's
//! colour as its border. See `docs/sidebar-spec.md`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use super::sidebar::{format_age, tokens};
use super::sidebar_view::{
    AppKitMetrics, Align, EDGE_TOLERANCE, LIST_BOTTOM, LIST_TOP, ListHit, PAD_H, Rect, Shared, Style, TextMetrics,
    draw_text, draw_wrapped, fill_dot, fill_round, stroke_icon, stroke_round,
};
use crate::pane::PaneId;

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
const NOTE_OPTION_GAP: f64 = 10.0;
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

// ---------------------------------------------------------------
// The view model
// ---------------------------------------------------------------

/// One question as the sidebar card shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AskCardVm {
    /// A stable, cheap key for the hit enum: the question id hashed. The
    /// view can compare hits without cloning a string on every mouse move.
    pub key: u64,
    pub id: String,
    /// The agent or session that asked.
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
    /// `OK` / `No` when it declared fewer.
    pub options: [String; 2],
    pub created_at: u64,
    /// The question's conversation has no live pane in Kova and no live
    /// Claude process: the note says `Session ended`.
    pub ended: bool,
    /// Put off until `back_at`.
    pub snoozed: bool,
    /// `20:00`, local time.
    pub back_at: String,
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
        let first = question.actions.first().cloned().unwrap_or_else(|| "OK".to_string());
        let second = question.actions.get(1).cloned().unwrap_or_else(|| "No".to_string());
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
            message: question.message.clone(),
            options: [first, second],
            created_at: question.asked_at,
            ended: pane.is_none() && !live,
            snoozed,
            back_at: if snoozed { local_hhmm(question.snoozed_until) } else { String::new() },
        }
    }
}

/// What the view keeps between ticks: the cards read from the board, the
/// folded state of the snoozed section, the card whose snooze row is open,
/// the card being sent, and the highlighted card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AsksState {
    pub cards: Vec<AskCardVm>,
    pub folded: bool,
    /// The question id whose snooze row is open.
    pub snooze_open: Option<String>,
    pub transient: Option<TransientAsk>,
    pub highlight: Option<Highlight>,
}

impl Default for AsksState {
    fn default() -> Self {
        AsksState { cards: Vec::new(), folded: true, snooze_open: None, transient: None, highlight: None }
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

    /// A new read of the board. The transient answer survives it: it is the
    /// card that just left the board (or is about to).
    pub fn set_cards(&mut self, cards: Vec<AskCardVm>) {
        self.cards = cards;
    }

    /// Drop the transient card once the board no longer holds it (it becomes
    /// `Sent`, for `SENT_LINGER_MS`) or once a sending click gave up. Clear a
    /// highlight that ran out. `waiting` is the keys the board still holds.
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
    /// Focus the pane the question came from.
    Open(PaneId),
    /// Open or close the three-choice snooze row.
    SnoozeMenu(u64),
    Snooze(u64, SnoozeChoice),
    /// Fold or unfold the `Snoozed N` section.
    ToggleSnoozed,
    /// Cancel a snooze: the question is open now.
    ShowNow(u64),
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
        AsksHit::Brief(_) => {
            state.snooze_open = None;
            None
        }
        AsksHit::Open(_) => None,
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
    }
}

fn answer(state: &mut AsksState, key: u64, option: usize, now_ms: u64) -> Option<AskAction> {
    if state.transient.as_ref().is_some_and(|t| t.card.key == key) {
        return None;
    }
    let card = state.card(key)?.clone();
    state.transient = Some(TransientAsk {
        card: card.clone(),
        phase: TransientPhase::Sending,
        until_ms: now_ms + SENDING_TIMEOUT_MS,
    });
    Some(AskAction::Answer { id: card.id, action: card.options[option].clone(), text: String::new() })
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
    pub content_h: f64,
}

impl AsksLayout {
    /// The button under `(x, y)`, if any. A press on the card's body is not a
    /// target: nothing happens there.
    pub fn hit(&self, x: f64, y: f64) -> Option<AsksHit> {
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
    out.content_h = y + LIST_BOTTOM;
    out
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
        let ok = Rect::new(inner_x, action_y, AB_W, AB_H);
        let ko = Rect::new(ok.right() + AB_GAP, action_y, AB_W, AB_H);
        let brief = Rect::new(ko.right() + AB_GAP, action_y, AB_W, AB_H);
        buttons.push((AsksHit::Ok(card.key), ok));
        buttons.push((AsksHit::Ko(card.key), ko));
        buttons.push((AsksHit::Brief(card.key), brief));
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
/// while the Asks list is up.
pub(super) fn draw(sh: &Shared, state: &AsksState, layout: &AsksLayout, hovered: ListHit, pressed: ListHit) {
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
}

fn draw_card(sh: &Shared, m: &AppKitMetrics<'_>, card: &CardLayout, hovered: ListHit, pressed: ListHit) {
    let alpha = if card.faded { 0.55 } else { 1.0 };
    fill_round(&card.frame, CARD_RADIUS, tokens::TILE, alpha);
    let (border, width) =
        if card.highlighted { (tokens::SEL_RAIL, 1.5) } else { (tokens::BORDER_SUBTLE, 1.0) };
    stroke_round(&card.frame, CARD_RADIUS, width, border, alpha);

    draw_header(sh, m, card, alpha);
    draw_wrapped(sh, &card.card.message, &card.message, Style::Secondary, tokens::TEXT_PRIMARY, alpha);

    if let (Some(status), Some(phase)) = (card.status, card.phase) {
        let (icon, fg, text) = match phase {
            TransientPhase::Sending => (super::feather::Icon::Send, tokens::TEXT_TERTIARY, format!("Sending to {}...", card.card.from)),
            TransientPhase::Sent => (super::feather::Icon::CheckCircle, tokens::OK_DOT, format!("Sent to {}", card.card.from)),
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
    } else if !card.faded {
        draw_options(sh, m, card, alpha);
    }
    if card.faded {
        draw_text(
            sh,
            &format!("back at {}", card.card.back_at),
            &card.note,
            Style::Hint,
            tokens::TEXT_TERTIARY,
            alpha,
            Align::Left,
            false,
        );
    }

    for (hit, rect) in &card.buttons {
        draw_button(sh, *hit, rect, alpha, hovered, pressed);
    }
}

fn draw_header(sh: &Shared, m: &AppKitMetrics<'_>, card: &CardLayout, alpha: f64) {
    let header = &card.header;
    fill_dot(header.x + 4.0, header.y + header.h / 2.0, STATE_DOT_D, tokens::AWAITING, alpha);
    let age_w = m.width(&card.card.age, Style::Number);
    let age = Rect::new(header.right() - age_w, header.y, age_w, header.h);
    let text_x = header.x + HEADER_TEXT_X;
    let room = (age.x - 8.0 - text_x).max(0.0);
    let tab_w = if card.card.tab.is_empty() {
        0.0
    } else {
        m.width(&card.card.tab, Style::Summary).min(room * 0.45)
    };
    let from_w = if tab_w > 0.0 { (room - tab_w - HEADER_TAB_GAP).max(0.0) } else { room };
    draw_text(
        sh,
        &card.card.from,
        &Rect::new(text_x, header.y, from_w, header.h),
        Style::Title,
        tokens::TEXT_PRIMARY,
        alpha,
        Align::Left,
        false,
    );
    if tab_w > 0.0 {
        draw_text(
            sh,
            &card.card.tab,
            &Rect::new(text_x + from_w + HEADER_TAB_GAP, header.y, tab_w, header.h),
            Style::Summary,
            tokens::TEXT_TERTIARY,
            alpha,
            Align::Left,
            false,
        );
    }
    let age_fg = if card.card.urgent { tokens::WARN_TEXT } else { tokens::TEXT_TERTIARY };
    draw_text(sh, &card.card.age, &age, Style::Number, age_fg, alpha, Align::Right, false);
}

fn draw_options(sh: &Shared, m: &AppKitMetrics<'_>, card: &CardLayout, alpha: f64) {
    let note = &card.note;
    let mut right = note.right();
    if card.card.ended {
        let label = "Session ended";
        let text_w = m.width(label, Style::Hint);
        let width = NOTE_ICON + NOTE_ICON_GAP + text_w;
        let x = (right - width).max(note.x);
        stroke_icon(
            super::feather::Icon::Info,
            &Rect::new(x, note.y, NOTE_ICON, note.h),
            NOTE_ICON,
            tokens::TEXT_TERTIARY,
            alpha,
        );
        draw_text(
            sh,
            label,
            &Rect::new(x + NOTE_ICON + NOTE_ICON_GAP, note.y, text_w, note.h),
            Style::Hint,
            tokens::TEXT_TERTIARY,
            alpha,
            Align::Left,
            false,
        );
        right = x - 8.0;
    }
    let avail = (right - note.x).max(0.0);
    let col = (avail - NOTE_OPTION_GAP) / 2.0;
    let text_w = (col - NOTE_ICON - NOTE_ICON_GAP).max(0.0);
    stroke_icon(
        super::feather::Icon::Check,
        &Rect::new(note.x, note.y, NOTE_ICON, note.h),
        NOTE_ICON,
        tokens::OK_DOT,
        alpha,
    );
    draw_text(
        sh,
        &card.card.options[0],
        &Rect::new(note.x + NOTE_ICON + NOTE_ICON_GAP, note.y, text_w, note.h),
        Style::Hint,
        tokens::TEXT_TERTIARY,
        alpha,
        Align::Left,
        false,
    );
    let second_x = note.x + col + NOTE_OPTION_GAP;
    stroke_icon(
        super::feather::Icon::X,
        &Rect::new(second_x, note.y, NOTE_ICON, note.h),
        NOTE_ICON,
        tokens::INTERRUPT,
        alpha,
    );
    draw_text(
        sh,
        &card.card.options[1],
        &Rect::new(second_x + NOTE_ICON + NOTE_ICON_GAP, note.y, text_w, note.h),
        Style::Hint,
        tokens::TEXT_TERTIARY,
        alpha,
        Align::Left,
        false,
    );
}

fn draw_button(sh: &Shared, hit: AsksHit, rect: &Rect, alpha: f64, hovered: ListHit, pressed: ListHit) {
    let hot = matches!(hovered, ListHit::Ask(h) if h == hit);
    let down = matches!(pressed, ListHit::Ask(h) if h == hit);
    match hit {
        AsksHit::Ok(_) | AsksHit::Ko(_) | AsksHit::Brief(_) => {
            let ground = if down || hot { tokens::TILE_PRESSED } else { tokens::TILE_HOVER };
            fill_round(rect, AB_RADIUS, ground, alpha);
            let (icon, fg) = match hit {
                AsksHit::Ok(_) => (super::feather::Icon::Check, tokens::OK_DOT),
                AsksHit::Ko(_) => (super::feather::Icon::X, tokens::INTERRUPT),
                _ => (super::feather::Icon::Edit, tokens::TEXT_PRIMARY),
            };
            stroke_icon(icon, rect, AB_ICON, fg, alpha);
        }
        AsksHit::Open(_) | AsksHit::SnoozeMenu(_) => {
            if hot || down {
                fill_round(rect, AB_RADIUS, tokens::WHITE, if down { 0.16 } else { 0.10 });
            }
            let icon = if matches!(hit, AsksHit::Open(_)) {
                super::feather::Icon::Open
            } else {
                super::feather::Icon::Moon
            };
            let fg = if hot || down { tokens::TEXT_PRIMARY } else { tokens::TEXT_TERTIARY };
            stroke_icon(icon, rect, GHOST_ICON, fg, alpha);
        }
        AsksHit::Snooze(_, choice) => {
            let ground = if down || hot { tokens::TILE_PRESSED } else { tokens::TILE_HOVER };
            fill_round(rect, SNOOZE_RADIUS, ground, alpha);
            draw_text(sh, choice.label(), rect, Style::Chip, tokens::TEXT_SECONDARY, alpha, Align::Center, false);
        }
        AsksHit::ShowNow(_) => {
            let ground = if down || hot { tokens::TILE_PRESSED } else { tokens::TILE_HOVER };
            fill_round(rect, SNOOZE_RADIUS, ground, alpha);
            draw_text(sh, "Show now", rect, Style::Chip, tokens::TEXT_PRIMARY, alpha, Align::Center, false);
        }
        AsksHit::ToggleSnoozed => {}
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
        fill_round(rect, 6.0, tokens::WHITE, if down { 0.16 } else { 0.08 });
    }
    let icon = if folded { super::feather::Icon::ChevronRight } else { super::feather::Icon::ChevronDown };
    let chevron = Rect::new(rect.x + 2.0, rect.y + (rect.h - SNOOZE_CHEVRON) / 2.0, SNOOZE_CHEVRON, SNOOZE_CHEVRON);
    stroke_icon(icon, &chevron, SNOOZE_CHEVRON, tokens::TEXT_TERTIARY, 1.0);
    let label = format!("Snoozed {count}");
    let w = m.width(&label, Style::Chip);
    draw_text(
        sh,
        &label,
        &Rect::new(rect.x + SNOOZE_TEXT_X, rect.y, w.max(0.0), rect.h),
        Style::Chip,
        tokens::TEXT_SECONDARY,
        1.0,
        Align::Left,
        false,
    );
}

fn draw_empty(sh: &Shared, rect: &Rect) {
    let icon = Rect::new(rect.x + (rect.w - EMPTY_ICON) / 2.0, rect.y + 8.0, EMPTY_ICON, EMPTY_ICON);
    stroke_icon(super::feather::Icon::CheckCircle, &icon, EMPTY_ICON, tokens::TEXT_TERTIARY, 0.7);
    draw_text(
        sh,
        "No open questions",
        &Rect::new(rect.x, icon.bottom() + 10.0, rect.w, 20.0),
        Style::Title,
        tokens::TEXT_SECONDARY,
        1.0,
        Align::Center,
        false,
    );
    draw_text(
        sh,
        "Agents ask here when they need you",
        &Rect::new(rect.x, icon.bottom() + 32.0, rect.w, 16.0),
        Style::Hint,
        tokens::TEXT_TERTIARY,
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
            created_at: 100,
            ended: false,
            snoozed: false,
            back_at: String::new(),
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
}
