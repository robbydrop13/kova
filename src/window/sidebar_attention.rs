//! The attention queue in the Asks view (`crate::attention`): its `now`
//! entries at the top, with the questions, under `À toi maintenant (N)`;
//! `Au prochain point (N)` folded below the cards, grouped by topic; and
//! `Pour mémoire (N)`, a count line whose rows are titles only. A row says
//! what it is (the title), what it wants (`needs`), who asks and since when,
//! and carries its own buttons. Nothing at all shows while the queue is
//! empty: a healthy queue takes no space.
//!
//! Pure like `sidebar_asks`: `build` turns the read into rows, `layout_top`
//! and `layout_bottom` into rects, `AttnLayout::hit` a point into an
//! `AttnHit`; `sidebar_asks` places them around its cards and draws them
//! with `draw`, `sidebar_ui` runs the buttons.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;

use super::feather::Icon;
use super::side_panel::PanelTarget;
use super::sidebar::{format_age, tokens};
use super::sidebar_view::{
    draw_text, draw_wrapped, fill_round, stroke_icon, stroke_round, Align, ListHit, Rect, Shared, Style, TextMetrics,
};
use crate::attention::{Entry, Level, Outcome, Verb};

// ---------------------------------------------------------------
// Metrics (points)
// ---------------------------------------------------------------

/// A section header: the chevron (folded sections), the label and count.
const SECTION_H: f64 = 24.0;
const SECTION_GAP: f64 = 8.0;
const CHEVRON: f64 = 12.0;
const SECTION_TEXT_X: f64 = 20.0;
/// A topic's label inside the open `Au prochain point`.
const TOPIC_H: f64 = 18.0;
/// A row: the card look of the questions, tighter.
const ROW_RADIUS: f64 = 8.0;
const ROW_PAD_V: f64 = 8.0;
const ROW_PAD_H: f64 = 10.0;
const ROW_GAP: f64 = 6.0;
const TITLE_H: f64 = 18.0;
const NEEDS_LINE_H: f64 = 15.0;
const NEEDS_MAX_LINES: usize = 3;
const LINE_GAP: f64 = 4.0;
/// The bottom line: who asks on the left, the buttons on the right.
const FOOT_H: f64 = 22.0;
const BTN_PAD: f64 = 8.0;
const BTN_GAP: f64 = 6.0;
const BTN_RADIUS: f64 = 6.0;
/// A `Pour mémoire` row: one line, no card.
const MEMO_H: f64 = 20.0;

pub const NOW_TITLE: &str = "À toi maintenant";
pub const POINT_TITLE: &str = "Au prochain point";
pub const MEMO_TITLE: &str = "Pour mémoire";
/// The topic of the entries that name none, last in `Au prochain point`.
pub const NO_TOPIC: &str = "Divers";
const OPEN_LABEL: &str = "Ouvrir";
const RUNNING: &str = "\u{2026}";
const SENT: &str = "Envoyé";

// ---------------------------------------------------------------
// The view model
// ---------------------------------------------------------------

/// A button a row can carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    /// Show the entry's link or file in the side panel. Changes nothing in
    /// the queue.
    Open,
    /// Run a command on the entry (`crate::attention::perform`).
    Verb(Verb),
}

impl Button {
    pub fn label(self) -> &'static str {
        match self {
            Button::Open => OPEN_LABEL,
            Button::Verb(verb) => verb.label(),
        }
    }
}

/// Where a row's click stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RowState {
    #[default]
    Idle,
    Running,
    Sent,
    Failed(String),
}

/// One entry as its row shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttnRow {
    /// The id hashed, for the hit enum.
    pub key: u64,
    pub id: String,
    pub title: String,
    pub needs: String,
    pub from: String,
    /// `4m`, `2h`; empty when the date did not read.
    pub age: String,
    pub since: u64,
    pub buttons: Vec<Button>,
    /// What `Ouvrir` shows, when the action is a link or a file the panel
    /// can render.
    pub open: Option<PanelTarget>,
    pub state: RowState,
}

/// The whole queue, split by level.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttentionVm {
    /// The queue is configured. Off, the Asks view is exactly as it was.
    pub enabled: bool,
    /// Oldest first: the one waiting longest is the most urgent.
    pub now: Vec<AttnRow>,
    /// By topic, the topic with the oldest entry first, `NO_TOPIC` last.
    pub point: Vec<(String, Vec<AttnRow>)>,
    /// Newest first.
    pub memo: Vec<AttnRow>,
}

impl AttentionVm {
    pub fn point_count(&self) -> usize {
        self.point.iter().map(|(_, rows)| rows.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.now.is_empty() && self.point.is_empty() && self.memo.is_empty()
    }

    /// Every row, for the click that names one by key.
    pub fn row(&self, key: u64) -> Option<&AttnRow> {
        self.now
            .iter()
            .chain(self.point.iter().flat_map(|(_, rows)| rows.iter()))
            .chain(self.memo.iter())
            .find(|row| row.key == key)
    }
}

/// The folded state of the two lower sections. Both start folded: they can
/// wait, and the view is about what cannot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AttnFold {
    pub point_open: bool,
    pub memo_open: bool,
}

pub fn row_key(id: &str) -> u64 {
    let mut h = DefaultHasher::new();
    ("attention", id).hash(&mut h);
    h.finish()
}

/// What `Ouvrir` would show for an entry's action: the side panel's own rule
/// (`side_panel::target_for`: an http(s) link, or an existing `.md` / `.html`
/// file, `~/` read as home), after the `#anchor` of a file is stripped. No
/// working folder: a relative path names nothing here. Plain words show
/// nothing.
pub fn open_target(action: &str, home: Option<&Path>, exists: &dyn Fn(&Path) -> bool) -> Option<PanelTarget> {
    let action = action.trim();
    if crate::ask::is_panel_url(action) {
        return Some(PanelTarget::Web(action.to_string()));
    }
    let file = match action.rfind('#') {
        Some(at) if !action[at..].contains('/') => &action[..at],
        _ => action,
    };
    super::side_panel::target_for(file, None, home, exists)
}

/// The buttons a row carries, in order: `Ouvrir` when there is something to
/// show, then by kind, `Envoyer` and `Ignorer` for what an agent would have
/// done alone, `Fait` for a decision, a review or a test. A verb whose
/// command is not configured is left out.
pub fn buttons(kind: &str, open: bool, can_act: bool, can_close: bool) -> Vec<Button> {
    let mut out = Vec::new();
    if open {
        out.push(Button::Open);
    }
    match kind {
        "would" => {
            if can_act {
                out.push(Button::Verb(Verb::Send));
            }
            if can_close {
                out.push(Button::Verb(Verb::Ignore));
            }
        }
        "decision" | "review" | "test" if can_close => out.push(Button::Verb(Verb::Done)),
        _ => {}
    }
    out
}

/// The rows of a read, at `now` (epoch seconds), with the clicks' states.
/// An entry closed from here is left out until the next read drops it.
pub fn build(
    entries: Option<&[Entry]>,
    outcomes: &HashMap<String, Outcome>,
    now: u64,
    can_act: bool,
    can_close: bool,
    home: Option<&Path>,
    exists: &dyn Fn(&Path) -> bool,
) -> AttentionVm {
    let Some(entries) = entries else { return AttentionVm::default() };
    let mut vm = AttentionVm { enabled: true, ..Default::default() };
    let mut point: Vec<(String, Vec<AttnRow>)> = Vec::new();
    for entry in entries {
        let state = match outcomes.get(&entry.id) {
            Some(Outcome::Closed) => continue,
            Some(Outcome::Running) => RowState::Running,
            Some(Outcome::Sent) => RowState::Sent,
            Some(Outcome::Failed { words, .. }) => RowState::Failed(words.clone()),
            None => RowState::Idle,
        };
        let open = entry.action.as_deref().and_then(|action| open_target(action, home, exists));
        let row = AttnRow {
            key: row_key(&entry.id),
            id: entry.id.clone(),
            title: entry.title.clone(),
            needs: entry.needs.clone().unwrap_or_default(),
            from: entry.from.clone(),
            age: if entry.since == 0 { String::new() } else { format_age(now.saturating_sub(entry.since)) },
            since: entry.since,
            buttons: buttons(&entry.kind, open.is_some(), can_act, can_close),
            open,
            state,
        };
        match entry.level {
            Level::Now => vm.now.push(row),
            Level::Memo => vm.memo.push(row),
            Level::Point => {
                let topic = entry.topic.clone().unwrap_or_else(|| NO_TOPIC.to_string());
                match point.iter_mut().find(|(t, _)| *t == topic) {
                    Some((_, rows)) => rows.push(row),
                    None => point.push((topic, vec![row])),
                }
            }
        }
    }
    let oldest = |a: &AttnRow, b: &AttnRow| a.since.cmp(&b.since).then_with(|| a.id.cmp(&b.id));
    vm.now.sort_by(oldest);
    vm.memo.sort_by(|a, b| oldest(b, a));
    for (_, rows) in &mut point {
        rows.sort_by(oldest);
    }
    point.sort_by(|(ta, a), (tb, b)| {
        (ta == NO_TOPIC).cmp(&(tb == NO_TOPIC)).then_with(|| a[0].since.cmp(&b[0].since)).then_with(|| ta.cmp(tb))
    });
    vm.point = point;
    vm
}

// ---------------------------------------------------------------
// Hits and layout
// ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttnHit {
    Button(u64, Button),
    TogglePoint,
    ToggleMemo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Now,
    Point,
    Memo,
}

/// One row laid out: its frame, and the lines inside it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowLayout {
    pub row: AttnRow,
    pub frame: Rect,
    pub title: Rect,
    pub needs: Option<Rect>,
    /// The bottom line, from and the buttons. `None` on a memo row.
    pub foot: Option<Rect>,
    pub memo: bool,
}

/// A part of the queue laid out: the top (`À toi maintenant` and its rows)
/// or the bottom (the two folded sections).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AttnLayout {
    pub headers: Vec<(Section, usize, Rect)>,
    pub topics: Vec<(String, Rect)>,
    pub rows: Vec<RowLayout>,
    pub buttons: Vec<(AttnHit, Rect)>,
}

impl AttnLayout {
    pub fn hit(&self, x: f64, y: f64) -> Option<AttnHit> {
        if let Some((hit, _)) = self.buttons.iter().find(|(_, rect)| rect.contains(x, y)) {
            return Some(*hit);
        }
        self.headers.iter().find(|(_, _, rect)| rect.contains(x, y)).and_then(|(section, _, _)| match section {
            Section::Point => Some(AttnHit::TogglePoint),
            Section::Memo => Some(AttnHit::ToggleMemo),
            Section::Now => None,
        })
    }

    /// Every clickable box, for the hand cursor.
    pub fn button_rects(&self) -> Vec<Rect> {
        let mut rects: Vec<Rect> = self.buttons.iter().map(|(_, rect)| *rect).collect();
        rects.extend(
            self.headers.iter().filter(|(section, _, _)| *section != Section::Now).map(|(_, _, rect)| *rect),
        );
        rects
    }
}

/// `À toi maintenant (N)` and the queue's `now` rows, from `y`. `asks` is the
/// number of question cards the section also holds, so N is every row under
/// the header. Returns the layout and where the cards go next. Nothing at
/// all, not even the header, while N is zero.
pub fn layout_top(vm: &AttentionVm, asks: usize, x: f64, y: f64, w: f64, m: &dyn TextMetrics) -> (AttnLayout, f64) {
    let mut out = AttnLayout::default();
    let total = vm.now.len() + asks;
    if !vm.enabled || total == 0 || vm.is_empty() {
        return (out, y);
    }
    out.headers.push((Section::Now, total, Rect::new(x, y, w, SECTION_H)));
    let mut y = y + SECTION_H;
    for row in &vm.now {
        y = place_row(&mut out, row, x, y, w, m) + ROW_GAP;
    }
    (out, y)
}

/// The two folded sections, from `y`: `Au prochain point (N)` and `Pour
/// mémoire (N)`, each only when it has a row. Returns the layout and the
/// bottom.
pub fn layout_bottom(vm: &AttentionVm, fold: AttnFold, x: f64, y: f64, w: f64, m: &dyn TextMetrics) -> (AttnLayout, f64) {
    let mut out = AttnLayout::default();
    let mut y = y;
    if !vm.enabled {
        return (out, y);
    }
    let point = vm.point_count();
    if point > 0 {
        y += SECTION_GAP;
        out.headers.push((Section::Point, point, Rect::new(x, y, w, SECTION_H)));
        y += SECTION_H;
        if fold.point_open {
            for (topic, rows) in &vm.point {
                out.topics.push((topic.clone(), Rect::new(x, y, w, TOPIC_H)));
                y += TOPIC_H;
                for row in rows {
                    y = place_row(&mut out, row, x, y, w, m) + ROW_GAP;
                }
            }
        }
    }
    if !vm.memo.is_empty() {
        y += SECTION_GAP;
        out.headers.push((Section::Memo, vm.memo.len(), Rect::new(x, y, w, SECTION_H)));
        y += SECTION_H;
        if fold.memo_open {
            for row in &vm.memo {
                let frame = Rect::new(x, y, w, MEMO_H);
                out.rows.push(RowLayout {
                    row: row.clone(),
                    frame,
                    title: Rect::new(x + SECTION_TEXT_X, y, (w - SECTION_TEXT_X).max(0.0), MEMO_H),
                    needs: None,
                    foot: None,
                    memo: true,
                });
                y += MEMO_H;
            }
        }
    }
    (out, y)
}

/// One `now` or `point` row from `y`; returns its bottom. The buttons sit at
/// the right of the bottom line, right to left in reverse, so the first one
/// is the leftmost; a row whose click runs, or was sent, has none.
fn place_row(out: &mut AttnLayout, row: &AttnRow, x: f64, y: f64, w: f64, m: &dyn TextMetrics) -> f64 {
    let inner_x = x + ROW_PAD_H;
    let inner_w = (w - 2.0 * ROW_PAD_H).max(0.0);
    let title = Rect::new(inner_x, y + ROW_PAD_V, inner_w, TITLE_H);
    let mut next = title.bottom();
    let needs = (!row.needs.is_empty()).then(|| {
        let lines = m.lines(&row.needs, Style::Secondary, inner_w, NEEDS_MAX_LINES).max(1);
        let rect = Rect::new(inner_x, next + LINE_GAP / 2.0, inner_w, lines as f64 * NEEDS_LINE_H);
        next = rect.bottom();
        rect
    });
    let foot = Rect::new(inner_x, next + LINE_GAP, inner_w, FOOT_H);
    if matches!(row.state, RowState::Idle | RowState::Failed(_)) {
        let mut right = foot.right();
        for button in row.buttons.iter().rev() {
            let bw = m.width(button.label(), Style::Chip) + 2.0 * BTN_PAD;
            // A narrow sidebar drops the leftmost buttons (Ouvrir first)
            // rather than draw them outside the row.
            if right - bw < foot.x {
                break;
            }
            let rect = Rect::new(right - bw, foot.y, bw, FOOT_H);
            out.buttons.push((AttnHit::Button(row.key, *button), rect));
            right = rect.x - BTN_GAP;
        }
    }
    let bottom = foot.bottom() + ROW_PAD_V;
    out.rows.push(RowLayout {
        row: row.clone(),
        frame: Rect::new(x, y, w, bottom - y),
        title,
        needs,
        foot: Some(foot),
        memo: false,
    });
    bottom
}

// ---------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------

/// Draw one part of the queue. `fold` gives the chevrons their direction.
pub(super) fn draw(sh: &Shared, layout: &AttnLayout, fold: AttnFold, hovered: ListHit, pressed: ListHit) {
    let m = super::sidebar_view::AppKitMetrics(sh);
    for (section, count, rect) in &layout.headers {
        let (title, open) = match section {
            Section::Now => (NOW_TITLE, true),
            Section::Point => (POINT_TITLE, fold.point_open),
            Section::Memo => (MEMO_TITLE, fold.memo_open),
        };
        if *section != Section::Now {
            let hit = if *section == Section::Point { AttnHit::TogglePoint } else { AttnHit::ToggleMemo };
            let hot = matches!(hovered, ListHit::Ask(super::sidebar_asks::AsksHit::Attention(h)) if h == hit);
            let down = matches!(pressed, ListHit::Ask(super::sidebar_asks::AsksHit::Attention(h)) if h == hit);
            if hot || down {
                fill_round(rect, 6.0, tokens::get().white, if down { 0.16 } else { 0.08 });
            }
            let icon = if open { Icon::ChevronDown } else { Icon::ChevronRight };
            let chevron = Rect::new(rect.x + 2.0, rect.y + (rect.h - CHEVRON) / 2.0, CHEVRON, CHEVRON);
            stroke_icon(icon, &chevron, CHEVRON, tokens::get().text_tertiary, 1.0);
        }
        let label = format!("{title} ({count})");
        let text_x = if *section == Section::Now { rect.x + 2.0 } else { rect.x + SECTION_TEXT_X };
        draw_text(
            sh,
            &label,
            &Rect::new(text_x, rect.y, (rect.right() - text_x).max(0.0), rect.h),
            Style::Chip,
            tokens::get().text_secondary,
            1.0,
            Align::Left,
            false,
        );
    }
    for (topic, rect) in &layout.topics {
        let r = Rect::new(rect.x + SECTION_TEXT_X, rect.y, (rect.w - SECTION_TEXT_X).max(0.0), rect.h);
        draw_text(sh, topic, &r, Style::Hint, tokens::get().text_tertiary, 1.0, Align::Left, false);
    }
    for row in &layout.rows {
        draw_row(sh, &m, row, layout, hovered, pressed);
    }
}

fn draw_row(
    sh: &Shared,
    m: &dyn TextMetrics,
    row: &RowLayout,
    layout: &AttnLayout,
    hovered: ListHit,
    pressed: ListHit,
) {
    let r = &row.row;
    if row.memo {
        let age_w = m.width(&r.age, Style::Number);
        let title = Rect::new(row.title.x, row.title.y, (row.title.w - age_w - 6.0).max(0.0), row.title.h);
        draw_text(sh, &r.title, &title, Style::Secondary, tokens::get().text_secondary, 1.0, Align::Left, false);
        draw_text(sh, &r.age, &row.title, Style::Number, tokens::get().text_tertiary, 1.0, Align::Right, false);
        return;
    }
    fill_round(&row.frame, ROW_RADIUS, tokens::get().tile, 1.0);
    stroke_round(&row.frame, ROW_RADIUS, 1.0, tokens::get().border_subtle, 1.0);

    let age_w = m.width(&r.age, Style::Number);
    let title = Rect::new(row.title.x, row.title.y, (row.title.w - age_w - 8.0).max(0.0), row.title.h);
    draw_text(sh, &r.title, &title, Style::Title, tokens::get().text_primary, 1.0, Align::Left, false);
    draw_text(sh, &r.age, &row.title, Style::Number, tokens::get().text_tertiary, 1.0, Align::Right, false);
    if let Some(needs) = row.needs {
        draw_wrapped(sh, &r.needs, &needs, Style::Secondary, tokens::get().text_secondary, 1.0);
    }
    let Some(foot) = row.foot else { return };
    let mine: Vec<&(AttnHit, Rect)> =
        layout.buttons.iter().filter(|(hit, _)| matches!(hit, AttnHit::Button(key, _) if *key == r.key)).collect();
    let left_end = mine.iter().map(|(_, rect)| rect.x).fold(foot.right(), f64::min) - BTN_GAP;
    let (words, ink) = match &r.state {
        RowState::Failed(words) => (words.clone(), tokens::get().crit_text),
        RowState::Running => (format!("{}  {RUNNING}", r.from), tokens::get().text_tertiary),
        RowState::Sent => (format!("{} \u{b7} {SENT}", r.from), tokens::get().ok_dot),
        RowState::Idle => (r.from.clone(), tokens::get().text_tertiary),
    };
    let words_rect = Rect::new(foot.x, foot.y, (left_end - foot.x).max(0.0), foot.h);
    draw_text(sh, &words, &words_rect, Style::Hint, ink, 1.0, Align::Left, false);
    for (hit, rect) in mine {
        let AttnHit::Button(_, button) = hit else { continue };
        let hot = matches!(hovered, ListHit::Ask(super::sidebar_asks::AsksHit::Attention(h)) if h == *hit);
        let down = matches!(pressed, ListHit::Ask(super::sidebar_asks::AsksHit::Attention(h)) if h == *hit);
        // `Ouvrir` is what the row is for: the accent ground of the
        // decision card's Open. The others are the small tile buttons.
        if *button == Button::Open {
            let ground = if down || hot { tokens::get().accent_pressed } else { tokens::get().accent };
            fill_round(rect, BTN_RADIUS, ground, 1.0);
            draw_text(sh, button.label(), rect, Style::Chip, tokens::get().text_on_fill, 1.0, Align::Center, false);
        } else {
            let ground = if down || hot { tokens::get().tile_pressed } else { tokens::get().tile_hover };
            fill_round(rect, BTN_RADIUS, ground, 1.0);
            draw_text(sh, button.label(), rect, Style::Chip, tokens::get().text_primary, 1.0, Align::Center, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct FakeMetrics;

    impl TextMetrics for FakeMetrics {
        fn width(&self, text: &str, _style: Style) -> f64 {
            text.chars().count() as f64 * 6.0
        }

        fn lines(&self, text: &str, _style: Style, width: f64, max: usize) -> usize {
            ((text.chars().count() as f64 * 6.0 / width).ceil() as usize).clamp(1, max)
        }
    }

    const NOW: u64 = 1_793_900_000;

    fn entries() -> Vec<Entry> {
        crate::attention::parse(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/attention/list.json")))
            .expect("the fixture")
    }

    fn exists_none(_: &Path) -> bool {
        false
    }

    fn vm_with(outcomes: &HashMap<String, Outcome>, exists: &dyn Fn(&Path) -> bool) -> AttentionVm {
        build(Some(&entries()), outcomes, NOW, true, true, None, exists)
    }

    #[test]
    fn rows_are_split_by_level_and_point_is_grouped_by_topic() {
        let vm = vm_with(&HashMap::new(), &exists_none);
        assert!(vm.enabled);
        let ids = |rows: &[AttnRow]| rows.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&vm.now), ["a-100001"]);
        let topics: Vec<&str> = vm.point.iter().map(|(t, _)| t.as_str()).collect();
        // `launch` holds the older point entry (14:00), `supplier` the newer.
        assert_eq!(topics, ["launch", "supplier"]);
        assert_eq!(vm.point_count(), 2);
        // Newest first; the entry whose date did not read is the oldest.
        assert_eq!(ids(&vm.memo), ["a-100004", "a-100005"]);
        assert_eq!(vm.memo[1].age, "");
        assert_eq!(vm.now[0].needs, "Pick one of the two plans");
        assert_eq!(vm.now[0].from, "alpha-manager");
    }

    #[test]
    fn entries_without_a_topic_go_last_under_one_label() {
        let mut list = entries();
        for e in &mut list {
            if e.id == "a-100003" {
                e.topic = None;
                e.since = 1;
            }
        }
        let vm = build(Some(&list), &HashMap::new(), NOW, true, true, None, &exists_none);
        let topics: Vec<&str> = vm.point.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(topics, ["supplier", NO_TOPIC]);
    }

    #[test]
    fn the_buttons_follow_the_kind_and_what_is_configured() {
        assert_eq!(buttons("would", false, true, true), [Button::Verb(Verb::Send), Button::Verb(Verb::Ignore)]);
        assert_eq!(buttons("decision", true, true, true), [Button::Open, Button::Verb(Verb::Done)]);
        assert_eq!(buttons("review", false, true, true), [Button::Verb(Verb::Done)]);
        assert_eq!(buttons("test", false, true, true), [Button::Verb(Verb::Done)]);
        assert_eq!(buttons("blocked", true, true, true), [Button::Open]);
        assert!(buttons("info", false, true, true).is_empty());
        // No close command: no Fait, no Ignorer; no act command: no Envoyer.
        assert!(buttons("decision", false, true, false).is_empty());
        assert_eq!(buttons("would", false, false, true), [Button::Verb(Verb::Ignore)]);
    }

    #[test]
    fn ouvrir_takes_a_link_or_an_existing_markdown_or_html_file() {
        let yes = |_: &Path| true;
        assert_eq!(
            open_target("https://example.com/a", None, &exists_none),
            Some(PanelTarget::Web("https://example.com/a".into()))
        );
        assert_eq!(
            open_target("/tmp/x/draft notes.md#P1", None, &yes),
            Some(PanelTarget::Markdown(PathBuf::from("/tmp/x/draft notes.md")))
        );
        assert_eq!(open_target("/tmp/x/page.html", None, &yes), Some(PanelTarget::Html(PathBuf::from("/tmp/x/page.html"))));
        assert_eq!(
            open_target("~/notes/a.md", Some(Path::new("/Users/me")), &yes),
            Some(PanelTarget::Markdown(PathBuf::from("/Users/me/notes/a.md")))
        );
        // Missing, relative, another type, or plain words: nothing to open.
        assert_eq!(open_target("/tmp/x/draft.md", None, &exists_none), None);
        assert_eq!(open_target("artifacts/a.md", None, &yes), None);
        assert_eq!(open_target("/tmp/x/a.pdf", None, &yes), None);
        assert_eq!(open_target("call the bank", None, &yes), None);
        assert_eq!(open_target("https://example.com/a b", None, &yes), None);
        assert_eq!(open_target("file:///tmp/x/a.md", None, &yes), Some(PanelTarget::Markdown(PathBuf::from("/tmp/x/a.md"))));
        // The anchor is checked off the file name only: a `#` in a folder stays.
        let seen = std::cell::RefCell::new(Vec::new());
        let record = |p: &Path| {
            seen.borrow_mut().push(p.to_path_buf());
            true
        };
        open_target("/tmp/a#b/c.md", None, &record);
        assert_eq!(seen.borrow()[0], PathBuf::from("/tmp/a#b/c.md"));
    }

    #[test]
    fn a_click_in_flight_or_sent_hides_the_buttons_and_a_closed_row_leaves() {
        let mut outcomes = HashMap::new();
        outcomes.insert("a-100001".to_string(), Outcome::Running);
        outcomes.insert("a-100002".to_string(), Outcome::Sent);
        outcomes.insert("a-100003".to_string(), Outcome::Closed);
        let vm = vm_with(&outcomes, &exists_none);
        assert_eq!(vm.now[0].state, RowState::Running);
        assert_eq!(vm.point_count(), 1, "the closed entry is gone before the next read");
        let fold = AttnFold { point_open: true, memo_open: false };
        let (top, _) = layout_top(&vm, 0, 12.0, 6.0, 256.0, &FakeMetrics);
        assert!(top.buttons.is_empty(), "a running click offers nothing");
        let (bottom, _) = layout_bottom(&vm, fold, 12.0, 100.0, 256.0, &FakeMetrics);
        assert_eq!(bottom.rows[0].row.state, RowState::Sent);
        assert!(bottom.buttons.is_empty(), "a sent row cannot send twice");
    }

    #[test]
    fn each_count_is_the_length_of_the_list_under_it() {
        let vm = vm_with(&HashMap::new(), &exists_none);
        let open = AttnFold { point_open: true, memo_open: true };
        // The top counts the queue's `now` rows and the question cards.
        let (top, y) = layout_top(&vm, 2, 12.0, 6.0, 256.0, &FakeMetrics);
        assert_eq!(top.headers, vec![(Section::Now, 3, Rect::new(12.0, 6.0, 256.0, SECTION_H))]);
        assert_eq!(top.rows.len(), 1);
        assert!(y > 6.0 + SECTION_H);
        let (bottom, _) = layout_bottom(&vm, open, 12.0, y, 256.0, &FakeMetrics);
        let counts: Vec<(Section, usize)> = bottom.headers.iter().map(|(s, n, _)| (*s, *n)).collect();
        assert_eq!(counts, [(Section::Point, 2), (Section::Memo, 2)]);
        let point_rows = bottom.rows.iter().filter(|r| !r.memo).count();
        let memo_rows = bottom.rows.iter().filter(|r| r.memo).count();
        assert_eq!((point_rows, memo_rows), (2, 2));
        assert_eq!(bottom.topics.len(), 2);
        // Folded: the headers only, and they toggle.
        let (folded, _) = layout_bottom(&vm, AttnFold::default(), 12.0, y, 256.0, &FakeMetrics);
        assert!(folded.rows.is_empty());
        let (_, _, point) = folded.headers[0];
        assert_eq!(folded.hit(point.x + 30.0, point.y + 5.0), Some(AttnHit::TogglePoint));
        let (_, _, memo) = folded.headers[1];
        assert_eq!(folded.hit(memo.x + 30.0, memo.y + 5.0), Some(AttnHit::ToggleMemo));
    }

    #[test]
    fn a_rows_buttons_sit_on_its_bottom_line_in_order() {
        let mut list = entries();
        list.retain(|e| e.id == "a-100002");
        list[0].level = Level::Now;
        let vm = build(Some(&list), &HashMap::new(), NOW, true, true, None, &exists_none);
        let (top, _) = layout_top(&vm, 0, 12.0, 6.0, 256.0, &FakeMetrics);
        let key = row_key("a-100002");
        let hits: Vec<AttnHit> = {
            let mut b = top.buttons.clone();
            b.sort_by(|a, c| a.1.x.total_cmp(&c.1.x));
            b.into_iter().map(|(h, _)| h).collect()
        };
        assert_eq!(hits, [AttnHit::Button(key, Button::Verb(Verb::Send)), AttnHit::Button(key, Button::Verb(Verb::Ignore))]);
        let foot = top.rows[0].foot.expect("a foot line");
        for (hit, rect) in &top.buttons {
            assert!(rect.y == foot.y && rect.right() <= foot.right() + 0.01);
            assert_eq!(top.hit(rect.x + 1.0, rect.y + 1.0), Some(*hit));
        }
    }

    #[test]
    fn a_narrow_row_drops_its_leftmost_buttons_instead_of_overflowing() {
        let row = AttnRow {
            key: 1,
            id: "a-1".into(),
            title: "t".into(),
            buttons: vec![Button::Open, Button::Verb(Verb::Send), Button::Verb(Verb::Ignore)],
            ..Default::default()
        };
        let vm = AttentionVm { enabled: true, now: vec![row], ..Default::default() };
        // 150 pt of row: 130 pt of bottom line, room for Ignorer and Envoyer
        // (58 pt each with their padding) but not Ouvrir.
        let (top, _) = layout_top(&vm, 0, 0.0, 0.0, 150.0, &FakeMetrics);
        let foot = top.rows[0].foot.expect("a foot line");
        let kept: Vec<Button> = top.buttons.iter().map(|(hit, _)| match hit {
            AttnHit::Button(_, b) => *b,
            _ => unreachable!(),
        }).collect();
        assert_eq!(kept, [Button::Verb(Verb::Ignore), Button::Verb(Verb::Send)]);
        assert!(top.buttons.iter().all(|(_, r)| r.x >= foot.x));
    }

    #[test]
    fn an_empty_queue_takes_no_space_and_the_feature_off_shows_nothing() {
        let empty = build(Some(&[]), &HashMap::new(), NOW, true, true, None, &exists_none);
        assert!(empty.enabled && empty.is_empty());
        let (top, y) = layout_top(&empty, 3, 12.0, 6.0, 256.0, &FakeMetrics);
        assert!(top.headers.is_empty() && y == 6.0, "the questions alone keep their old look");
        let (bottom, y) = layout_bottom(&empty, AttnFold::default(), 12.0, 50.0, 256.0, &FakeMetrics);
        assert!(bottom.headers.is_empty() && y == 50.0);
        let off = build(None, &HashMap::new(), NOW, true, true, None, &exists_none);
        assert!(!off.enabled);
        assert_eq!(layout_top(&off, 3, 12.0, 6.0, 256.0, &FakeMetrics).0, AttnLayout::default());
    }
}
