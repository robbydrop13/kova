//! The Scheduled view of the sidebar: the local routines grouped into
//! sections, each row a two-line summary with its state glyph, its next run,
//! and (under the mouse, or in the body it opens onto) the four actions
//! `routinectl` serves. Pure: `layout` turns the model's routines into rects,
//! `hit` turns a point into a `SchedHit`, `draw` paints one row.
//!
//! `sidebar_ui` runs the clicks and owns the state between ticks; `routines`
//! serves the actions on a background thread and reports back through its
//! queue. See `docs/sidebar-spec.md`.

use super::sidebar::tokens;
use super::feather::Icon;
use super::sidebar_model::RoutineVm;
use super::sidebar_view::{
    AppKitMetrics, Align, EDGE_TOLERANCE, LIST_BOTTOM, LIST_TOP, ListHit, PAD_H, Rect, Row, RowKind, Shared, Style,
    TextMetrics, draw_text, fill_dot, fill_round, stroke_icon, stroke_round,
};
use crate::routines::{ActionResult, Group, LastStatus, RoutineAction, When};

// ---------------------------------------------------------------
// Metrics (points)
// ---------------------------------------------------------------

/// A row: 40 pt, 5 above the two 15 pt lines and 5 under them, 6 either side.
pub const ROW_H: f64 = 40.0;
const ROW_PAD_X: f64 = 6.0;
const ROW_PAD_Y: f64 = 5.0;
const LINE_H: f64 = 15.0;
const ROW_RADIUS: f64 = 6.0;
const OPEN_RADIUS: f64 = 8.0;
const ROW_GAP: f64 = 2.0;
/// The state glyph's column, its icon, and where the text starts: 6 + 16 + 4.
const GLYPH_W: f64 = 16.0;
const GLYPH_D: f64 = 13.0;
const TEXT_X: f64 = 26.0;
/// The ghost icon buttons of the hover row, and the icon inside them.
const BTN: f64 = 22.0;
const BTN_ICON: f64 = 14.0;
const BTN_GAP: f64 = 2.0;
const BTN_RADIUS: f64 = 6.0;
/// A section header: the row, its chevron, the label after it, and the gap
/// before the count.
const SECTION_H: f64 = 24.0;
const SECTION_GAP: f64 = 8.0;
const SECTION_PAD_X: f64 = 6.0;
const SECTION_CHEVRON_X: f64 = 6.0;
const SECTION_LABEL_X: f64 = 24.0;
const SECTION_COUNT_GAP: f64 = 6.0;
const CHEVRON: f64 = 12.0;
/// The open row's body, indented 24 from the row's left edge: a key/value
/// grid (72 pt of label), then a row of small buttons.
pub const BODY_INDENT: f64 = 24.0;
const BODY_GAP: f64 = 4.0;
const KV_H: f64 = 17.0;
const KV_LABEL_W: f64 = 72.0;
const BODY_BTN_TOP_GAP: f64 = 4.0;
/// A body button: a real 24 pt button, its 12 pt glyph, the gap after it,
/// 6 pt of padding either side (the snooze chips'), 6 pt between buttons.
const BODY_BTN_H: f64 = 24.0;
const BODY_BTN_ICON: f64 = 12.0;
const BODY_BTN_ICON_GAP: f64 = 6.0;
const BODY_BTN_PAD: f64 = 6.0;
const BODY_BTN_GAP: f64 = 6.0;
/// The confirmation's buttons keep the wider padding they had.
const CONFIRM_BTN_PAD: f64 = 8.0;
const BODY_PAD_B: f64 = 8.0;
/// The inline delete confirmation: a crit wash, two lines and two buttons.
const CONFIRM_GAP: f64 = 8.0;
const CONFIRM_PAD: f64 = 8.0;
const CONFIRM_TITLE_H: f64 = 15.0;
const CONFIRM_SUB_GAP: f64 = 2.0;
const CONFIRM_SUB_H: f64 = 15.0;
const CONFIRM_BTN_GAP: f64 = 6.0;
pub const CONFIRM_H: f64 = 2.0 * CONFIRM_PAD + CONFIRM_TITLE_H + CONFIRM_SUB_GAP + CONFIRM_SUB_H + CONFIRM_BTN_GAP
    + BTN;
/// How long a failed action's words stay on the row.
pub const ERROR_MS: u64 = 10_000;

// ---------------------------------------------------------------
// Hit targets
// ---------------------------------------------------------------

/// Every clickable part of one row: the hover row's icon buttons, the body's
/// labelled buttons, and the confirmation's two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedButton {
    Run,
    Log,
    Pause,
    Resume,
    Edit,
    Delete,
    /// The confirmation's `Delete`: the one that really deletes.
    ConfirmDelete,
    /// The confirmation's `Cancel`.
    ConfirmCancel,
}

impl SchedButton {
    /// The words a labelled button shows (the body's row, the confirmation).
    pub fn label(self) -> &'static str {
        match self {
            SchedButton::Run => "Run now",
            SchedButton::Log => "Open log",
            SchedButton::Pause => "Pause",
            SchedButton::Resume => "Resume",
            SchedButton::Edit => "Edit prompt",
            SchedButton::Delete | SchedButton::ConfirmDelete => "Delete",
            SchedButton::ConfirmCancel => "Cancel",
        }
    }

    /// The glyph of the hover row's icon button.
    pub fn icon(self) -> Option<Icon> {
        match self {
            SchedButton::Run | SchedButton::Resume => Some(Icon::Play),
            SchedButton::Log => Some(Icon::FileText),
            SchedButton::Pause => Some(Icon::Pause),
            SchedButton::Edit => Some(Icon::Edit),
            SchedButton::Delete => Some(Icon::Trash),
            SchedButton::ConfirmDelete | SchedButton::ConfirmCancel => None,
        }
    }

    /// Drawn in the crit colour: the delete, and what confirms it.
    pub fn crit(self) -> bool {
        matches!(self, SchedButton::Delete | SchedButton::ConfirmDelete)
    }

    /// The type style of a labelled button: the body's are the chip, the
    /// confirmation's filled `Delete` one weight heavier.
    pub fn style(self) -> Style {
        if self == SchedButton::ConfirmDelete { Style::ChipStrong } else { Style::Chip }
    }

    /// The `routinectl` verb this button runs, if it runs one.
    pub fn action(self) -> Option<RoutineAction> {
        match self {
            SchedButton::Run => Some(RoutineAction::Run),
            SchedButton::Pause => Some(RoutineAction::Pause),
            SchedButton::Resume => Some(RoutineAction::Resume),
            SchedButton::ConfirmDelete => Some(RoutineAction::Delete),
            SchedButton::Log | SchedButton::Edit | SchedButton::Delete | SchedButton::ConfirmCancel => None,
        }
    }
}

/// Where a button sits, which decides how it is painted: the hover row's icon
/// strip, the body's labelled row, or inside the confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedSpot {
    Hover,
    Body,
    Confirm,
}

/// What a point of the Scheduled list lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedHit {
    /// A section header: fold or unfold it.
    Section(Group),
    /// The row itself: open or close its body.
    Row(usize),
    /// One of its buttons, by index into the model's routines.
    Button(usize, SchedButton),
}

/// The hover row's buttons for a state: a running routine can be paused, an
/// off one resumed, and both can be deleted.
const ACTIVE_BUTTONS: [SchedButton; 5] =
    [SchedButton::Run, SchedButton::Log, SchedButton::Pause, SchedButton::Edit, SchedButton::Delete];
const OFF_BUTTONS: [SchedButton; 2] = [SchedButton::Resume, SchedButton::Delete];

/// The buttons a row offers, hover row and body alike.
pub fn buttons(r: &RoutineVm) -> &'static [SchedButton] {
    if r.group == Group::Off { &OFF_BUTTONS } else { &ACTIVE_BUTTONS }
}

// ---------------------------------------------------------------
// View state
// ---------------------------------------------------------------

/// What the view keeps between ticks: which sections are folded, the row
/// whose delete is being confirmed, the actions in flight and the ones that
/// failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedState {
    /// Folded sections. `Off` starts folded: it is the one a glance does not
    /// need.
    pub folded: Vec<Group>,
    /// The routine whose inline delete confirmation is open.
    pub confirm: Option<String>,
    /// Actions in flight, by routine name: the row says what it is doing.
    pub busy: Vec<(String, RoutineAction)>,
    /// A failed action's words, and the epoch millis they stop showing at.
    pub errors: Vec<(String, String, u64)>,
    /// The clock the meta row's `now 18:45` reads. `None` before the first
    /// tick, so a test can leave it out.
    pub now: Option<When>,
}

impl Default for SchedState {
    fn default() -> Self {
        SchedState { folded: vec![Group::Off], confirm: None, busy: Vec::new(), errors: Vec::new(), now: None }
    }
}

impl SchedState {
    pub fn is_folded(&self, group: Group) -> bool {
        self.folded.contains(&group)
    }

    pub fn toggle(&mut self, group: Group) {
        match self.folded.iter().position(|g| *g == group) {
            Some(i) => {
                self.folded.remove(i);
            }
            None => self.folded.push(group),
        }
    }

    pub fn busy_action(&self, name: &str) -> Option<RoutineAction> {
        self.busy.iter().find(|(n, _)| n == name).map(|(_, a)| *a)
    }

    /// The action left for this row: it says so and ignores its buttons.
    pub fn start(&mut self, name: &str, action: RoutineAction) {
        if self.busy_action(name).is_none() {
            self.busy.push((name.to_string(), action));
        }
    }

    /// Apply what a background action reported: the row stops saying what it
    /// is doing, and a failure keeps its words for `ERROR_MS`.
    pub fn finish(&mut self, result: &ActionResult, at_ms: u64) {
        self.busy.retain(|(n, _)| n != &result.name);
        if let Err(why) = &result.result {
            let words = format!("Could not {}: {why}", result.action.verb());
            self.failed(&result.name, words, at_ms);
        }
    }

    /// A failure that never reached `routinectl`: a log or a prompt outside
    /// the routines directory, say.
    pub fn failed(&mut self, name: &str, words: String, at_ms: u64) {
        self.errors.retain(|(n, _, _)| n != name);
        self.errors.push((name.to_string(), words, at_ms + ERROR_MS));
    }

    pub fn error(&self, name: &str) -> Option<&str> {
        self.errors.iter().find(|(n, _, _)| n == name).map(|(_, w, _)| w.as_str())
    }

    /// Drop the words whose time is up.
    pub fn prune(&mut self, at_ms: u64) {
        self.errors.retain(|(_, _, until)| *until > at_ms);
    }

    pub fn open_confirm(&mut self, name: &str) {
        self.confirm = Some(name.to_string());
    }

    pub fn close_confirm(&mut self) {
        self.confirm = None;
    }
}

/// What the row's second line says while an action runs.
pub fn busy_words(action: RoutineAction) -> &'static str {
    match action {
        RoutineAction::Run => "Running now\u{2026}",
        RoutineAction::Pause => "Pausing\u{2026}",
        RoutineAction::Resume => "Resuming\u{2026}",
        RoutineAction::Delete => "Deleting\u{2026}",
    }
}

/// The ink the second line takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Muted,
    Crit,
    Info,
}

/// The row's second line, from the schedule and how the last run ended.
pub fn line_two(r: &RoutineVm) -> (String, Tone) {
    let schedule = &r.schedule_words;
    if r.group == Group::Off {
        return (format!("Paused \u{b7} was {schedule}"), Tone::Muted);
    }
    // routinectl may know a code without the times: then the row says what
    // it knows and leaves the age out rather than trailing a separator.
    let ago = if r.ago_words.is_empty() { String::new() } else { format!(" {}", r.ago_words) };
    match r.last_status {
        LastStatus::Running => (format!("{schedule} \u{b7} running"), Tone::Info),
        LastStatus::Failed(_) => (format!("{schedule} \u{b7} failed{ago}"), Tone::Crit),
        LastStatus::Ok => (format!("{schedule} \u{b7} ran{ago}"), Tone::Muted),
        LastStatus::Never => (format!("{schedule} \u{b7} never run"), Tone::Muted),
    }
}

/// The body's five key/value lines, in the order that answers "what will this
/// actually do".
pub fn body_rows(r: &RoutineVm) -> [(&'static str, String); 5] {
    [
        ("Runs at", r.schedule_words.clone()),
        ("Next", next_value(r)),
        ("Last run", last_value(r)),
        ("Where", short_home(&r.cwd)),
        ("Prompt", format!("prompts/{}.md", r.name)),
    ]
}

fn next_value(r: &RoutineVm) -> String {
    if r.group == Group::Off {
        return "Paused".to_string();
    }
    if r.next_at.is_none() {
        return "Manual".to_string();
    }
    if r.in_words.is_empty() { r.next_words.clone() } else { format!("{} \u{b7} {}", r.next_words, r.in_words) }
}

fn last_value(r: &RoutineVm) -> String {
    match r.last_status {
        LastStatus::Running => "Running".to_string(),
        LastStatus::Never => "Never".to_string(),
        LastStatus::Ok if r.ago_words.is_empty() => "Ok".to_string(),
        LastStatus::Ok => format!("Ok \u{b7} {}", r.ago_words),
        LastStatus::Failed(code) if r.ago_words.is_empty() => format!("Failed \u{b7} exit {code}"),
        LastStatus::Failed(code) => format!("Failed {} \u{b7} exit {code}", r.ago_words),
    }
}

/// The ink of one of the body's values: `Last run` turns crit when the run it
/// reports failed, the one line of the grid that carries a verdict.
pub fn body_value_ink(label: &str, r: &RoutineVm) -> [f32; 3] {
    if label == "Last run" && matches!(r.last_status, LastStatus::Failed(_)) {
        tokens::get().crit_text
    } else {
        tokens::get().text_primary
    }
}

/// A path with the home directory written `~`.
pub fn short_home(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return path.to_string();
    }
    match path.strip_prefix(&home) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// The meta row's left side: `N active · K failed`.
pub fn meta_left(routines: &[RoutineVm]) -> Vec<(String, bool)> {
    let active = routines.iter().filter(|r| r.group != Group::Off).count();
    let failed = routines.iter().filter(|r| r.group == Group::NeedsAttention).count();
    let mut out = vec![(format!("{active} active"), false)];
    if failed > 0 {
        out.push((" \u{b7} ".to_string(), false));
        out.push((format!("{failed} failed"), true));
    }
    out
}

/// The meta row's right side: the clock the list is phrased against.
pub fn clock_words(state: &SchedState) -> Option<String> {
    state.now.map(|n| format!("now {:02}:{:02}", n.hour, n.minute))
}

/// The width the body's contents get: the row's, indented 24 and padded.
fn body_width(frame: &Rect) -> f64 {
    (frame.right() - ROW_PAD_X - (frame.x + BODY_INDENT)).max(40.0)
}

/// The height the body of an open row needs, `buttons_h` measured by
/// `body_buttons`. Shared by the layout and the draw so the two cannot
/// disagree.
pub fn body_height(confirm: bool, buttons_h: f64) -> f64 {
    let mut h = BODY_GAP + 5.0 * KV_H + BODY_BTN_TOP_GAP + buttons_h + BODY_PAD_B;
    if confirm {
        h += CONFIRM_GAP + CONFIRM_H;
    }
    h
}

// ---------------------------------------------------------------
// Layout
// ---------------------------------------------------------------

/// One section, laid out: its header, its count and whether it is folded.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub group: Group,
    pub header: Rect,
    pub count: usize,
    pub folded: bool,
}

/// The Scheduled list's sections at one width. The rows themselves are the
/// list's own (`ListLayout::rows`): one list of rows, one hit test, one draw.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchedLayout {
    pub sections: Vec<Section>,
    pub hint_y: Option<f64>,
    pub content_h: f64,
}

/// The routines of one group, next run first; a manual routine has none and
/// comes last, by name.
fn order(a: &RoutineVm, b: &RoutineVm) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a.next_at, b.next_at) {
        (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.name.cmp(&b.name)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.name.cmp(&b.name),
    }
}

/// Lay the Scheduled list out at `width`. An empty list keeps the hint slot
/// the other empty lists borrow.
pub fn layout(routines: &[RoutineVm], state: &SchedState, width: f64, m: &dyn TextMetrics) -> (SchedLayout, Vec<Row>) {
    let x0 = PAD_H;
    let x1 = (width - (PAD_H - EDGE_TOLERANCE)).max(x0 + 40.0);
    let mut out = SchedLayout::default();
    let mut rows: Vec<Row> = Vec::new();
    let mut y = LIST_TOP;
    if routines.is_empty() {
        out.hint_y = Some(y);
        out.content_h = y + 20.0 + LIST_BOTTOM;
        return (out, rows);
    }
    for group in Group::ALL {
        let mut members: Vec<usize> =
            routines.iter().enumerate().filter(|(_, r)| r.group == group).map(|(i, _)| i).collect();
        if members.is_empty() {
            continue;
        }
        members.sort_by(|&a, &b| order(&routines[a], &routines[b]));
        if !out.sections.is_empty() {
            y += SECTION_GAP;
        }
        let folded = state.is_folded(group);
        let header = Rect::new(x0, y, x1 - x0, SECTION_H);
        out.sections.push(Section { group, header, count: members.len(), folded });
        rows.push(Row {
            kind: RowKind::SchedHeader { group, count: members.len() },
            frame: header,
            glyphs: Vec::new(),
            actions: Vec::new(),
            detail: Rect::default(),
            answer: None,
            sched: Vec::new(),
            sched_confirm: None,
        });
        y += SECTION_H;
        if folded {
            continue;
        }
        for (nth, index) in members.into_iter().enumerate() {
            if nth > 0 || !rows.is_empty() {
                y += ROW_GAP;
            }
            let r = &routines[index];
            let confirm = state.confirm.as_deref() == Some(r.name.as_str());
            let buttons_h =
                if r.expanded { body_buttons(r, 0.0, 0.0, body_width(&Rect::new(x0, y, x1 - x0, 0.0)), m).1 } else { 0.0 };
            let h = ROW_H + if r.expanded { body_height(confirm, buttons_h) } else { 0.0 };
            let frame = Rect::new(x0, y, x1 - x0, h);
            let mut row = Row {
                kind: RowKind::Routine { index },
                frame,
                glyphs: Vec::new(),
                actions: Vec::new(),
                detail: Rect::default(),
                answer: None,
                sched: Vec::new(),
                sched_confirm: None,
            };
            place_buttons(&mut row, r, confirm, m);
            rows.push(row);
            y += h;
        }
    }
    out.content_h = y + LIST_BOTTOM;
    (out, rows)
}

/// The clickable boxes of one row: the hover strip (always laid out, so the
/// text can stop before it and the hit test knows it), the body's labelled
/// row, and the confirmation's two buttons.
fn place_buttons(row: &mut Row, r: &RoutineVm, confirm: bool, m: &dyn TextMetrics) {
    let frame = row.frame;
    let mut x = frame.right() - ROW_PAD_X;
    let cy = frame.y + (ROW_H - BTN) / 2.0;
    if !confirm {
        for &button in buttons(r).iter().rev() {
            x -= BTN;
            row.sched.push((button, Rect::new(x, cy, BTN, BTN), SchedSpot::Hover));
            x -= BTN_GAP;
        }
    }
    if !r.expanded {
        return;
    }
    let body_x = frame.x + BODY_INDENT;
    let body_w = body_width(&frame);
    let y = frame.y + ROW_H + BODY_GAP + 5.0 * KV_H + BODY_BTN_TOP_GAP;
    let (body, buttons_h) = body_buttons(r, body_x, y, body_w, m);
    row.sched.extend(body.into_iter().map(|(button, rect)| (button, rect, SchedSpot::Body)));
    if !confirm {
        return;
    }
    let y = y + buttons_h + CONFIRM_GAP;
    let box_ = Rect::new(body_x, y, body_w, CONFIRM_H);
    row.sched_confirm = Some(box_);
    let button_y = box_.y + CONFIRM_PAD + CONFIRM_TITLE_H + CONFIRM_SUB_GAP + CONFIRM_SUB_H + CONFIRM_BTN_GAP;
    let mut right = box_.x + box_.w - CONFIRM_PAD;
    for button in [SchedButton::ConfirmDelete, SchedButton::ConfirmCancel] {
        let w = m.width(button.label(), button.style()) + 2.0 * CONFIRM_BTN_PAD;
        right -= w;
        row.sched.push((button, Rect::new(right, button_y, w, BTN), SchedSpot::Confirm));
        right -= CONFIRM_BTN_GAP;
    }
}

/// The body's labelled buttons, wrapped onto as many lines as `width` needs,
/// and the height those lines take: a narrow sidebar has no room for five
/// labels on one line.
fn body_buttons(r: &RoutineVm, x: f64, y: f64, width: f64, m: &dyn TextMetrics) -> (Vec<(SchedButton, Rect)>, f64) {
    let mut out: Vec<(SchedButton, Rect)> = Vec::with_capacity(5);
    let mut bx = x;
    let mut by = y;
    let mut lines = 1.0;
    for &button in buttons(r) {
        let w = body_button_width(button, m);
        if !out.is_empty() && bx + w > x + width {
            bx = x;
            by += BODY_BTN_H + BODY_BTN_GAP;
            lines += 1.0;
        }
        out.push((button, Rect::new(bx, by, w, BODY_BTN_H)));
        bx += w + BODY_BTN_GAP;
    }
    (out, lines * BODY_BTN_H + (lines - 1.0) * BODY_BTN_GAP)
}

/// The width one body button needs: its padding, its glyph, the gap after it
/// and its label. `draw_body_button` lays the same boxes out.
fn body_button_width(button: SchedButton, m: &dyn TextMetrics) -> f64 {
    let lead = if button.icon().is_some() { BODY_BTN_ICON + BODY_BTN_ICON_GAP } else { 0.0 };
    2.0 * BODY_BTN_PAD + lead + m.width(button.label(), Style::Chip)
}

/// What a point inside a scheduled row lands on: one of its buttons, the
/// confirmation, the row, or the section header.
pub(super) fn hit(row: &Row, x: f64, y: f64) -> Option<SchedHit> {
    if !row.frame.contains(x, y) {
        return None;
    }
    if let RowKind::Routine { index } = row.kind
        && let Some((button, _, _)) = row.sched.iter().find(|(_, rect, _)| rect.contains(x, y))
    {
        return Some(SchedHit::Button(index, *button));
    }
    match row.kind {
        RowKind::SchedHeader { group, .. } => Some(SchedHit::Section(group)),
        RowKind::Routine { index } => Some(SchedHit::Row(index)),
        _ => None,
    }
}

// ---------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------

fn tone_ink(tone: Tone) -> [f32; 3] {
    match tone {
        Tone::Muted => tokens::get().text_tertiary,
        Tone::Crit => tokens::get().crit_text,
        Tone::Info => tokens::get().info_text,
    }
}

/// Draw one row of the Scheduled list: a section header, or a routine.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    sh: &Shared,
    routines: &[RoutineVm],
    state: &SchedState,
    row: &Row,
    frame: &Rect,
    hovered: ListHit,
    pressed: ListHit,
    alpha: f64,
) {
    match row.kind {
        RowKind::SchedHeader { group, count } => draw_section(sh, group, count, state, frame, alpha),
        RowKind::Routine { index } => {
            if let Some(r) = routines.get(index) {
                draw_routine(sh, r, state, index, row, frame, hovered, pressed, alpha);
            }
        }
        _ => {}
    }
}

fn draw_section(sh: &Shared, group: Group, count: usize, state: &SchedState, frame: &Rect, alpha: f64) {
    let cy = frame.y + frame.h / 2.0;
    let cx = frame.x + SECTION_CHEVRON_X;
    let chevron = if state.is_folded(group) { Icon::ChevronRight } else { Icon::ChevronDown };
    stroke_icon(chevron, &Rect::new(cx, cy - CHEVRON / 2.0, CHEVRON, CHEVRON), CHEVRON, tokens::get().text_tertiary, alpha);
    let label_fg = if group == Group::NeedsAttention { tokens::get().crit_text } else { tokens::get().text_secondary };
    let x = frame.x + SECTION_LABEL_X;
    let w = draw_text_sh(sh, group.label(), &Rect::new(x, frame.y, frame.w, frame.h), Style::Chip, label_fg, alpha);
    let count_text = count.to_string();
    draw_text_sh(
        sh,
        &count_text,
        &Rect::new(x + w + SECTION_COUNT_GAP, frame.y, frame.w, frame.h),
        Style::Chip,
        tokens::get().text_tertiary,
        alpha,
    );
    if let Some(note) = group.note() {
        let w = AppKitMetrics(sh).width(note, Style::Hint);
        draw_text_sh(
            sh,
            note,
            &Rect::new(frame.right() - SECTION_PAD_X - w, frame.y, w, frame.h),
            Style::Hint,
            tokens::get().text_tertiary,
            alpha,
        );
    }
}

fn draw_text_sh(sh: &Shared, text: &str, r: &Rect, style: Style, c: [f32; 3], alpha: f64) -> f64 {
    draw_text(sh, text, r, style, c, alpha, Align::Left, false)
}

#[allow(clippy::too_many_arguments)]
fn draw_routine(
    sh: &Shared,
    r: &RoutineVm,
    state: &SchedState,
    index: usize,
    row: &Row,
    frame: &Rect,
    hovered: ListHit,
    pressed: ListHit,
    alpha: f64,
) {
    // A routine switched off stays on the list, faded: it is not gone, it is
    // off, and that is exactly what is easy to forget.
    let a = if r.group == Group::Off { alpha * 0.55 } else { alpha };
    let hit = |b: SchedButton| ListHit::Sched(SchedHit::Button(index, b));
    let row_hovered = hovered == ListHit::Sched(SchedHit::Row(index))
        || row.sched.iter().any(|(b, _, _)| hovered == hit(*b));
    if r.expanded {
        fill_round(frame, OPEN_RADIUS, tokens::get().tile, a);
        stroke_round(frame, OPEN_RADIUS, 1.0, tokens::get().border_subtle, a);
    } else if row_hovered {
        fill_round(frame, ROW_RADIUS, tokens::get().tile_hover, a);
    }
    let cy = frame.y + ROW_H / 2.0;
    draw_glyph(r, frame.x + ROW_PAD_X + GLYPH_W / 2.0, cy, a);

    // The text column stops before the meta, or before the buttons that
    // replace it under the mouse.
    let mut right = frame.right() - ROW_PAD_X;
    if row_hovered {
        if let Some(x) = row.sched.iter().filter(|(_, _, s)| *s == SchedSpot::Hover).map(|(_, rect, _)| rect.x).fold(None, min_opt) {
            right = right.min(x - 4.0);
        }
    } else if r.group != Group::Off && r.next_at.is_some() {
        let m = AppKitMetrics(sh);
        let w = m.width(&r.next_words, Style::Secondary).max(m.width(&r.in_words, Style::Number));
        draw_text(sh, &r.next_words, &Rect::new(right - w, frame.y + 4.0, w, 16.0), Style::Secondary, tokens::get().text_secondary, a, Align::Right, false);
        draw_text(sh, &r.in_words, &Rect::new(right - w, frame.y + 20.0, w, LINE_H), Style::Number, tokens::get().text_tertiary, a, Align::Right, false);
        right -= w + 6.0;
    }
    let text_x = frame.x + TEXT_X;
    let text_w = (right - text_x).max(20.0);
    draw_text(sh, &r.name, &Rect::new(text_x, frame.y + ROW_PAD_Y, text_w, LINE_H), Style::Title, tokens::get().text_primary, a, Align::Left, false);

    let (line, tone) = match (state.busy_action(&r.name), state.error(&r.name)) {
        (Some(action), _) => (busy_words(action).to_string(), Tone::Muted),
        (None, Some(why)) => (why.to_string(), Tone::Crit),
        (None, None) => line_two(r),
    };
    draw_text(sh, &line, &Rect::new(text_x, frame.y + ROW_PAD_Y + LINE_H, text_w, LINE_H), Style::Secondary, tone_ink(tone), a, Align::Left, false);

    if row_hovered {
        for (button, rect, spot) in &row.sched {
            if *spot != SchedSpot::Hover {
                continue;
            }
            let hovered_here = hovered == hit(*button);
            let pressed_here = pressed == hit(*button);
            if pressed_here || hovered_here {
                fill_round(rect, BTN_RADIUS, tokens::get().white, if pressed_here { 0.16 } else { 0.10 } * a);
            }
            if let Some(icon) = button.icon() {
                let fg = if button.crit() { tokens::get().crit_text } else { tokens::get().text_secondary };
                stroke_icon(icon, rect, BTN_ICON, fg, a);
            }
        }
    }
    if !r.expanded {
        return;
    }
    draw_body(sh, r, row, frame, hovered, pressed, a, index);
}

fn min_opt(acc: Option<f64>, x: f64) -> Option<f64> {
    Some(acc.map_or(x, |a| a.min(x)))
}

fn draw_glyph(r: &RoutineVm, cx: f64, cy: f64, a: f64) {
    let box_ = Rect::new(cx - GLYPH_W / 2.0, cy - GLYPH_W / 2.0, GLYPH_W, GLYPH_W);
    let (icon, c) = match r.last_status {
        _ if r.group == Group::Off => (Icon::Power, tokens::get().text_tertiary),
        LastStatus::Ok => (Icon::Check, tokens::get().ok_dot),
        LastStatus::Failed(_) => (Icon::XCircle, tokens::get().error),
        LastStatus::Running => {
            fill_dot(cx, cy, 8.0, tokens::get().working, a);
            return;
        }
        LastStatus::Never => (Icon::Ring, tokens::get().text_tertiary),
    };
    stroke_icon(icon, &box_, GLYPH_D, c, a);
}

/// One of the body's labelled buttons: a 24 pt ground, its 12 pt glyph and
/// its label, the pair left-aligned like the ask card's action buttons.
/// `Delete` is the exception: no ground and crit ink, so the button that only
/// opens the confirmation does not wear the colour of the one that deletes.
fn draw_body_button(sh: &Shared, button: SchedButton, r: &Rect, hovered: bool, pressed: bool, a: f64) {
    let crit = button.crit();
    if !crit {
        let ground = if hovered || pressed { tokens::get().tile_pressed } else { tokens::get().tile_hover };
        fill_round(r, BTN_RADIUS, ground, a);
    }
    let fg = if crit { tokens::get().crit_text } else { tokens::get().text_primary };
    let mut x = r.x + BODY_BTN_PAD;
    if let Some(icon) = button.icon() {
        let icon_box = Rect::new(x, r.y + (r.h - BODY_BTN_ICON) / 2.0, BODY_BTN_ICON, BODY_BTN_ICON);
        stroke_icon(icon, &icon_box, BODY_BTN_ICON, fg, a);
        x = icon_box.right() + BODY_BTN_ICON_GAP;
    }
    let w = (r.right() - BODY_BTN_PAD - x).max(0.0);
    draw_text(sh, button.label(), &Rect::new(x, r.y, w, r.h), Style::Chip, fg, a, Align::Left, false);
}

#[allow(clippy::too_many_arguments)]
fn draw_body(
    sh: &Shared,
    r: &RoutineVm,
    row: &Row,
    frame: &Rect,
    hovered: ListHit,
    pressed: ListHit,
    a: f64,
    index: usize,
) {
    let hit = |b: SchedButton| ListHit::Sched(SchedHit::Button(index, b));
    let body_x = frame.x + BODY_INDENT;
    let body_w = body_width(frame);
    let mut y = frame.y + ROW_H + BODY_GAP;
    for (label, value) in body_rows(r) {
        draw_text(sh, label, &Rect::new(body_x, y, KV_LABEL_W, KV_H), Style::Secondary, tokens::get().text_tertiary, a, Align::Left, false);
        let ink = body_value_ink(label, r);
        draw_text(sh, &value, &Rect::new(body_x + KV_LABEL_W, y, (body_w - KV_LABEL_W).max(20.0), KV_H), Style::Secondary, ink, a, Align::Left, true);
        y += KV_H;
    }
    for (button, rect, spot) in &row.sched {
        if *spot != SchedSpot::Body {
            continue;
        }
        draw_body_button(sh, *button, rect, hovered == hit(*button), pressed == hit(*button), a);
    }
    let Some(box_) = row.sched_confirm else { return };
    // The confirmation: a crit wash, the question in bold, what it removes,
    // then Cancel and Delete (filled).
    fill_round(&box_, 6.0, tokens::get().crit_text, 0.12 * a);
    stroke_round(&box_, 6.0, 1.0, tokens::get().crit_text, 0.35 * a);
    let inner_x = box_.x + CONFIRM_PAD;
    let inner_w = (box_.w - 2.0 * CONFIRM_PAD).max(20.0);
    draw_text(sh, &format!("Delete {}?", r.name), &Rect::new(inner_x, box_.y + CONFIRM_PAD, inner_w, CONFIRM_TITLE_H), Style::Title, tokens::get().text_primary, a, Align::Left, false);
    draw_text(
        sh,
        "Removes its schedule and its prompt",
        &Rect::new(inner_x, box_.y + CONFIRM_PAD + CONFIRM_TITLE_H + CONFIRM_SUB_GAP, inner_w, CONFIRM_SUB_H),
        Style::Detail,
        tokens::get().text_tertiary,
        a,
        Align::Left,
        false,
    );
    for (button, rect, spot) in &row.sched {
        if *spot != SchedSpot::Confirm {
            continue;
        }
        let hovered_here = hovered == hit(*button);
        let pressed_here = pressed == hit(*button);
        if *button == SchedButton::ConfirmDelete {
            let ground = if hovered_here || pressed_here { tokens::get().crit_solid_hover } else { tokens::get().crit_solid };
            fill_round(rect, BTN_RADIUS, ground, a);
            draw_text(sh, button.label(), rect, Style::ChipStrong, tokens::get().text_on_fill, 1.0, Align::Center, false);
        } else {
            if hovered_here || pressed_here {
                fill_round(rect, BTN_RADIUS, tokens::get().white, if pressed_here { 0.16 } else { 0.10 } * a);
            }
            draw_text(sh, button.label(), rect, Style::Chip, tokens::get().text_secondary, a, Align::Center, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Six points per character.
    struct FakeMetrics;

    impl TextMetrics for FakeMetrics {
        fn width(&self, text: &str, _style: Style) -> f64 {
            text.chars().count() as f64 * 6.0
        }

        fn lines(&self, text: &str, _style: Style, width: f64, max: usize) -> usize {
            ((text.chars().count() as f64 * 6.0 / width).ceil() as usize).clamp(1, max)
        }
    }

    /// The K07 fixture's own clock, the one `routines` groups it at.
    const NOW: &str = "2026-11-03T20:30";

    fn at(s: &str) -> When {
        When::parse(s).expect("a fixed clock")
    }

    fn fixture() -> Vec<RoutineVm> {
        let now = at(NOW);
        crate::routines::fixture_list().iter().map(|r| RoutineVm::at(r, false, now)).collect()
    }

    fn index_of(routines: &[RoutineVm], name: &str) -> usize {
        routines.iter().position(|r| r.name == name).expect("a fixture routine")
    }

    fn row_of(rows: &[Row], index: usize) -> &Row {
        rows.iter().find(|r| r.kind == RowKind::Routine { index }).expect("a laid-out row")
    }

    #[test]
    fn sections_follow_the_fixture_at_a_fixed_clock() {
        let routines = fixture();
        let (sched, rows) = layout(&routines, &SchedState::default(), 280.0, &FakeMetrics);
        let groups: Vec<Group> = sched.sections.iter().map(|s| s.group).collect();
        assert_eq!(groups, [Group::NeedsAttention, Group::Today, Group::ThisWeek, Group::Later, Group::Off]);
        // Next run first inside a section, a manual routine last; the off
        // routine stays hidden behind its folded header.
        let names: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r.kind {
                RowKind::Routine { index } => Some(routines[index].name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            names,
            ["gamma-report", "beta-cleanup", "alpha-digest", "eta-week", "theta-edge", "zeta-later", "epsilon-manual"]
        );
        assert_eq!(sched.sections[0].header.h, SECTION_H);
        assert_eq!(sched.sections[0].count, 1);
        assert_eq!(sched.sections[1].count, 2);
    }

    #[test]
    fn off_starts_folded_and_a_header_toggles_its_section() {
        let routines = fixture();
        let mut state = SchedState::default();
        assert!(state.is_folded(Group::Off));
        let (folded, rows) = layout(&routines, &state, 280.0, &FakeMetrics);
        assert_eq!(rows.iter().filter(|r| matches!(r.kind, RowKind::SchedHeader { .. })).count(), 5);
        let delta = index_of(&routines, "delta-paused");
        assert!(rows.iter().all(|r| r.kind != RowKind::Routine { index: delta }));

        // Opened, the off routine joins the list and the content grows.
        state.toggle(Group::Off);
        let (open, rows) = layout(&routines, &state, 280.0, &FakeMetrics);
        assert!(!open.sections[4].folded);
        assert!(open.content_h > folded.content_h);
        let row = row_of(&rows, delta);
        assert_eq!(hit(row, row.frame.x + 2.0, row.frame.y + 2.0), Some(SchedHit::Row(delta)));

        // Today folded keeps its header and drops its two rows.
        state.toggle(Group::Today);
        let (today_folded, rows) = layout(&routines, &state, 280.0, &FakeMetrics);
        assert!(today_folded.sections[1].folded);
        assert!(rows.iter().all(|r| !matches!(r.kind, RowKind::Routine { index } if routines[index].group == Group::Today)));
        // Folding is a toggle, not a latch.
        state.toggle(Group::Today);
        let (_, rows) = layout(&routines, &state, 280.0, &FakeMetrics);
        assert!(rows.iter().any(|r| matches!(r.kind, RowKind::Routine { index } if routines[index].group == Group::Today)));
    }

    /// The height the body's labelled buttons take in a laid-out row.
    fn buttons_h(row: &Row) -> f64 {
        let body = row.sched.iter().filter(|(_, _, spot)| *spot == SchedSpot::Body);
        let top = body.clone().map(|(_, r, _)| r.y).fold(f64::INFINITY, f64::min);
        let bottom = body.map(|(_, r, _)| r.bottom()).fold(f64::NEG_INFINITY, f64::max);
        bottom - top
    }

    #[test]
    fn a_closed_row_is_forty_and_an_open_one_carries_its_body() {
        let mut routines = fixture();
        let alpha = index_of(&routines, "alpha-digest");
        let (_, rows) = layout(&routines, &SchedState::default(), 280.0, &FakeMetrics);
        assert_eq!(row_of(&rows, alpha).frame.h, ROW_H);
        assert_eq!(ROW_H, 40.0);

        routines[alpha].expanded = true;
        let (_, rows) = layout(&routines, &SchedState::default(), 280.0, &FakeMetrics);
        let row = row_of(&rows, alpha);
        assert_eq!(row.frame.h, ROW_H + body_height(false, buttons_h(row)));
        // Five key/value lines, the button row, and the padding around them.
        assert_eq!(
            body_height(false, BODY_BTN_H),
            BODY_GAP + 5.0 * KV_H + BODY_BTN_TOP_GAP + BODY_BTN_H + BODY_PAD_B
        );
        // Five labelled buttons, glyph included, do not fit one line in a
        // narrow sidebar: two lines at 280, one at 520.
        assert_eq!(buttons_h(row), 2.0 * BODY_BTN_H + BODY_BTN_GAP);
        let (_, wide) = layout(&routines, &SchedState::default(), 520.0, &FakeMetrics);
        assert_eq!(buttons_h(row_of(&wide, alpha)), BODY_BTN_H);

        // The confirmation adds its box to the open row.
        let mut state = SchedState::default();
        state.open_confirm("alpha-digest");
        let (_, rows) = layout(&routines, &state, 280.0, &FakeMetrics);
        let row = row_of(&rows, alpha);
        assert_eq!(row.frame.h, ROW_H + body_height(true, buttons_h(row)));
        assert!(body_height(true, BODY_BTN_H) > body_height(false, BODY_BTN_H));
    }

    #[test]
    fn a_scheduled_row_offers_five_actions_and_an_off_one_two() {
        let routines = fixture();
        let alpha = index_of(&routines, "alpha-digest");
        let delta = index_of(&routines, "delta-paused");
        assert_eq!(
            buttons(&routines[alpha]),
            [SchedButton::Run, SchedButton::Log, SchedButton::Pause, SchedButton::Edit, SchedButton::Delete]
        );
        assert_eq!(buttons(&routines[delta]), [SchedButton::Resume, SchedButton::Delete]);

        // Laid right to left on the row, ending at its right padding.
        let mut state = SchedState::default();
        state.toggle(Group::Off);
        let (_, rows) = layout(&routines, &state, 280.0, &FakeMetrics);
        let row = row_of(&rows, alpha);
        let hover: Vec<SchedButton> =
            row.sched.iter().filter(|(_, _, spot)| *spot == SchedSpot::Hover).map(|(b, _, _)| *b).collect();
        assert_eq!(
            hover,
            [SchedButton::Delete, SchedButton::Edit, SchedButton::Pause, SchedButton::Log, SchedButton::Run]
        );
        let delete =
            row.sched.iter().find(|(b, _, _)| *b == SchedButton::Delete).map(|(_, r, _)| *r).expect("a delete");
        assert_eq!(delete.right(), row.frame.right() - ROW_PAD_X);
        assert_eq!((delete.w, delete.h), (BTN, BTN));
        // The off row keeps two.
        let off = row_of(&rows, delta);
        assert_eq!(off.sched.iter().filter(|(_, _, s)| *s == SchedSpot::Hover).count(), 2);
    }

    #[test]
    fn every_button_and_the_confirmation_have_their_own_hit() {
        let mut routines = fixture();
        let alpha = index_of(&routines, "alpha-digest");
        routines[alpha].expanded = true;
        let mut state = SchedState::default();
        let (_, rows) = layout(&routines, &state, 280.0, &FakeMetrics);
        let row = row_of(&rows, alpha);
        // Every box, the hover strip and the body's row alike.
        for (button, rect, _) in &row.sched {
            assert_eq!(hit(row, rect.x + 2.0, rect.y + 2.0), Some(SchedHit::Button(alpha, *button)));
        }
        let body: Vec<SchedButton> =
            row.sched.iter().filter(|(_, _, s)| *s == SchedSpot::Body).map(|(b, _, _)| *b).collect();
        assert_eq!(body, [SchedButton::Run, SchedButton::Log, SchedButton::Pause, SchedButton::Edit, SchedButton::Delete]);
        // Anywhere else on the row opens it, body included; off it, nothing.
        assert_eq!(hit(row, row.frame.x + 2.0, row.frame.y + ROW_H / 2.0), Some(SchedHit::Row(alpha)));
        assert_eq!(hit(row, row.frame.x + BODY_INDENT, row.frame.bottom() - 2.0), Some(SchedHit::Row(alpha)));
        assert_eq!(hit(row, row.frame.x - 1.0, row.frame.y), None);

        // The confirmation replaces the hover strip and owns two buttons.
        state.open_confirm("alpha-digest");
        let (_, rows) = layout(&routines, &state, 280.0, &FakeMetrics);
        let row = row_of(&rows, alpha);
        assert!(row.sched.iter().all(|(_, _, s)| *s != SchedSpot::Hover));
        let box_ = row.sched_confirm.expect("the confirmation box");
        assert_eq!(hit(row, box_.x + 2.0, box_.y + 2.0), Some(SchedHit::Row(alpha)));
        for button in [SchedButton::ConfirmDelete, SchedButton::ConfirmCancel] {
            let rect = row.sched.iter().find(|(b, _, _)| *b == button).map(|(_, r, _)| *r).expect("its button");
            assert_eq!(hit(row, rect.x + 2.0, rect.y + 2.0), Some(SchedHit::Button(alpha, button)));
        }
    }

    #[test]
    fn a_body_button_holds_its_glyph_and_its_label() {
        let mut routines = fixture();
        let alpha = index_of(&routines, "alpha-digest");
        routines[alpha].expanded = true;
        let (_, rows) = layout(&routines, &SchedState::default(), 280.0, &FakeMetrics);
        let row = row_of(&rows, alpha);
        let body: Vec<(SchedButton, Rect)> =
            row.sched.iter().filter(|(_, _, s)| *s == SchedSpot::Body).map(|(b, r, _)| (*b, *r)).collect();
        assert_eq!(body.len(), 5);
        for (button, rect) in &body {
            assert_eq!(rect.h, BODY_BTN_H);
            assert_eq!(rect.w, body_button_width(*button, &FakeMetrics));
            let icon = Rect::new(
                rect.x + BODY_BTN_PAD,
                rect.y + (rect.h - BODY_BTN_ICON) / 2.0,
                BODY_BTN_ICON,
                BODY_BTN_ICON,
            );
            assert!(icon.right() <= rect.right(), "{button:?}: the glyph leaves its button");
            assert!(rect.y <= icon.y && icon.bottom() <= rect.bottom(), "{button:?}: the glyph spills out");
            let label_x = icon.right() + BODY_BTN_ICON_GAP;
            let label_w = FakeMetrics.width(button.label(), Style::Chip);
            assert!(
                label_x + label_w <= rect.right() - BODY_BTN_PAD + 1e-9,
                "{button:?}: {} does not fit its button",
                button.label()
            );
        }
        // 6 pt between two buttons on a line, the same left edge on the next
        // (FakeMetrics fits Run, Log and Pause on the first line at 280).
        let first = body[0].1;
        assert_eq!(body[1].1.x, first.right() + BODY_BTN_GAP);
        assert_eq!(body[3].1.x, first.x);
        assert!(body[3].1.y >= first.bottom() + BODY_BTN_GAP);
    }

    #[test]
    fn the_confirmation_deletes_in_the_solid_danger_colour() {
        // `#ED3726` solid, `#CC2314` under the mouse, white 11 semibold on it.
        assert_eq!(tokens::get().crit_solid, [0.929, 0.216, 0.149]);
        assert_eq!(tokens::get().crit_solid_hover, [0.800, 0.137, 0.078]);
        assert_eq!(SchedButton::ConfirmDelete.style(), Style::ChipStrong);
        assert_eq!(SchedButton::ConfirmDelete.style().size(), 11.0);
        assert_eq!(
            SchedButton::ConfirmDelete.style().weight(),
            crate::window::sidebar_view::Weight::Semibold
        );
        // Cancel stays the ghost chip, and the body's Delete is not the solid.
        assert_eq!(SchedButton::ConfirmCancel.style(), Style::Chip);
        assert_eq!(SchedButton::Delete.style(), Style::Chip);
    }

    #[test]
    fn a_failed_last_run_inks_its_value_crit() {
        let routines = fixture();
        let failed = index_of(&routines, "gamma-report");
        let ok = index_of(&routines, "alpha-digest");
        assert_eq!(body_value_ink("Last run", &routines[failed]), tokens::get().crit_text);
        assert_eq!(body_value_ink("Last run", &routines[ok]), tokens::get().text_primary);
        // Only that value: the rest of the grid keeps its ink.
        assert_eq!(body_value_ink("Next", &routines[failed]), tokens::get().text_primary);
        assert_eq!(body_value_ink("Runs at", &routines[failed]), tokens::get().text_primary);
    }

    #[test]
    fn a_running_action_says_what_it_is_doing() {
        let mut state = SchedState::default();
        assert_eq!(state.busy_action("alpha-digest"), None);
        state.start("alpha-digest", RoutineAction::Run);
        state.start("alpha-digest", RoutineAction::Pause);
        // The first action owns the row until it reports back.
        assert_eq!(state.busy_action("alpha-digest"), Some(RoutineAction::Run));
        assert_eq!(busy_words(RoutineAction::Run), "Running now\u{2026}");
        assert_eq!(busy_words(RoutineAction::Pause), "Pausing\u{2026}");
        assert_eq!(busy_words(RoutineAction::Resume), "Resuming\u{2026}");
        assert_eq!(busy_words(RoutineAction::Delete), "Deleting\u{2026}");
    }

    #[test]
    fn a_failure_keeps_its_words_for_ten_seconds() {
        let mut state = SchedState::default();
        state.start("alpha-digest", RoutineAction::Delete);
        let result = ActionResult {
            name: "alpha-digest".into(),
            action: RoutineAction::Delete,
            result: Err("no such routine".into()),
        };
        state.finish(&result, 1_000);
        assert_eq!(state.busy_action("alpha-digest"), None);
        assert_eq!(state.error("alpha-digest"), Some("Could not delete: no such routine"));
        state.prune(1_000 + ERROR_MS);
        assert_eq!(state.error("alpha-digest"), None);
        // A clean action leaves no words behind.
        let ok = ActionResult { name: "alpha-digest".into(), action: RoutineAction::Run, result: Ok(()) };
        state.finish(&ok, 1_000);
        assert_eq!(state.error("alpha-digest"), None);
    }

    #[test]
    fn the_second_line_says_how_the_last_run_ended() {
        let routines = fixture();
        let by = |name: &str| &routines[index_of(&routines, name)];
        assert_eq!(line_two(by("alpha-digest")), ("2 times a day \u{b7} ran 11 h ago".to_string(), Tone::Muted));
        assert_eq!(line_two(by("beta-cleanup")), ("every 30 min \u{b7} running".to_string(), Tone::Info));
        assert_eq!(line_two(by("gamma-report")), ("daily 21:00 \u{b7} failed 23 h ago".to_string(), Tone::Crit));
        assert_eq!(line_two(by("theta-edge")), ("once, 10 Nov 09:03 \u{b7} never run".to_string(), Tone::Muted));
        assert_eq!(line_two(by("delta-paused")), ("Paused \u{b7} was Mondays 07:12".to_string(), Tone::Muted));
    }

    #[test]
    fn the_meta_row_counts_the_active_and_the_failed() {
        let routines = fixture();
        assert_eq!(
            meta_left(&routines),
            vec![("7 active".to_string(), false), (" \u{b7} ".to_string(), false), ("1 failed".to_string(), true)]
        );
        let mut state = SchedState::default();
        assert_eq!(clock_words(&state), None);
        state.now = Some(at("2026-11-03T18:45"));
        assert_eq!(clock_words(&state), Some("now 18:45".to_string()));
    }

    #[test]
    fn the_body_says_runs_at_next_last_where_and_prompt() {
        let routines = fixture();
        let closed = &routines[index_of(&routines, "alpha-digest")];
        // The body is only read while the row is open: a closed row carries
        // no directory, which keeps the tick's comparison cheap.
        assert_eq!(closed.cwd, "");
        let alpha = &crate::routines::fixture_list()
            .into_iter()
            .find(|r| r.name == "alpha-digest")
            .map(|r| RoutineVm::at(&r, true, at(NOW)))
            .expect("a fixture routine");
        assert_eq!(
            body_rows(alpha),
            [
                ("Runs at", "2 times a day".to_string()),
                ("Next", "21:07 \u{b7} in 37 min".to_string()),
                ("Last run", "Ok \u{b7} 11 h ago".to_string()),
                ("Where", "/run/alpha".to_string()),
                ("Prompt", "prompts/alpha-digest.md".to_string()),
            ]
        );
        // A manual routine has no next run; an off one says it is paused.
        assert_eq!(body_rows(&routines[index_of(&routines, "epsilon-manual")])[1].1, "Manual");
        assert_eq!(body_rows(&routines[index_of(&routines, "delta-paused")])[1].1, "Paused");
        // A failure is reported with its exit code.
        assert_eq!(body_rows(&routines[index_of(&routines, "gamma-report")])[2].1, "Failed 23 h ago \u{b7} exit 2");
    }

    #[test]
    fn the_home_directory_reads_as_a_tilde() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(short_home(&home), "~");
        assert_eq!(short_home(&format!("{home}/project")), "~/project");
        assert_eq!(short_home("/run/alpha"), "/run/alpha");
    }
}
