//! The sidebar as an AppKit view: KovaLink's home screen in Inter, group
//! blocks, tiles with no fill of their own and their meta column, drawn with
//! Cocoa (`drawRect:`) next to the Metal
//! terminal view. Two classes: `SidebarView` owns the chrome (view switcher,
//! title row, meta row, Next pill, resize edge) and an `NSScrollView`;
//! `SidebarListView` is the scroll view's document and draws the groups and
//! tiles. Both share one `Shared` cell holding the `SidebarModel`, the
//! layouts and the transient mouse state.
//!
//! The layouts (`ListLayout`, `ChromeLayout`) are pure: points in, rects
//! out, text widths through the `TextMetrics` trait so tests run without a
//! window. Every action reaches the window's `KovaView` (`sidebar_ui.rs`)
//! through `kova_of`. See `docs/sidebar-spec.md`.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::{
    NSAffineTransformNSAppKitAdditions, NSApplication, NSBezierPath, NSColor, NSControl,
    NSControlTextEditingDelegate, NSCursor, NSEvent, NSEventMask, NSEventModifierFlags, NSFocusRingType, NSFont,
    NSGraphicsContext, NSImage, NSLineBreakMode, NSMutableParagraphStyle, NSScrollElasticity, NSScrollView,
    NSScrollerStyle, NSShadow, NSStringDrawing, NSStringDrawingOptions, NSStringNSExtendedStringDrawing, NSTextField,
    NSTextFieldDelegate, NSTextView, NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSAffineTransform, NSDictionary, NSNotification, NSObjectProtocol, NSString};

use super::feather::{self, Icon};
use super::needs::NeedKind;
use super::sidebar::{
    self, displacements, drag_bounds, next_slot, placeholder_y, tab_tint, tokens, NextPill, SidebarSort, Slot,
    SummaryRun, TileButton, TileState, RESUME_LABEL, START_CLAUDE_LABEL,
};
use super::sidebar_asks::{self, AsksHit, AsksLayout};
use super::sidebar_model::{need_rows_shown, GroupVm, NeedRowVm, SidebarList, SidebarModel, TileVm};
#[cfg(test)]
use super::sidebar_model::RoutineVm;
use super::sidebar_sched::{self, SchedButton, SchedHit, SchedLayout, SchedSpot};
use super::sidebar_agents::{self, AgentsHit, AgentsLayout};
use super::sidebar_ui::{PaneAction, TabAction};
use super::KovaView;
use crate::pane::{PaneId, TabId};

// ---------------------------------------------------------------
// Metrics (points)
// ---------------------------------------------------------------

/// The separator line at the sidebar's right edge, part of its width.
pub const SEP_W: f64 = 1.0;
/// Points either side of the separator that grab the resize handle.
pub(super) const EDGE_TOLERANCE: f64 = 4.0;
/// The top row, under the title bar: the traffic lights on the left and the
/// view switcher at its right end.
const TOP_H: f64 = 38.0;
/// The title row: the list's name on the left, the Next pill on the right.
const TITLE_H: f64 = 40.0;
/// The meta row: the summary on the left, the sort ghost on the right.
const META_H: f64 = 20.0;
/// The title's distance from the sidebar's left edge, and the least gap
/// kept between its end and the pill.
const TITLE_LEFT: f64 = 14.0;
const TITLE_GAP: f64 = 8.0;
/// The pill's distance from the sidebar's right edge, its height, and the
/// padding inside it either side of its content.
const PILL_INSET: f64 = 12.0;
const PILL_H: f64 = 24.0;
const PILL_PAD: f64 = 9.0;
/// Inside the pill: icon to label, and label to badge.
const PILL_ICON_GAP: f64 = 4.0;
const PILL_BADGE_GAP: f64 = 6.0;
const PILL_BADGE_H: f64 = 16.0;
/// Horizontal padding of the chrome and the list content.
pub(super) const PAD_H: f64 = 12.0;
pub(super) const LIST_TOP: f64 = 6.0;
pub(super) const LIST_BOTTOM: f64 = 12.0;
const GROUP_GAP: f64 = 8.0;
/// A group's card padding, above the header and under the last tile.
const GROUP_PAD_V: f64 = 4.0;
/// The gap between two rows of a group: header to first tile, tile to tile.
const ROW_GAP: f64 = 4.0;
const HEADER_H: f64 = 28.0;
/// Every row of a group, header included, is inset this far from the card's
/// edges: their grounds line up, hovered or selected.
const ROW_INSET: f64 = 8.0;
/// The header's hover ground: a rounded row, not a full pill.
const HEADER_RADIUS: f64 = 6.0;
/// The active group's card radius.
const PANEL_RADIUS: f64 = 8.0;
/// The chevron at the left of a header, then the tab's colour dot (filled,
/// the tab's colour 0..11) and its click box. The zone holds the 12 pt
/// chevron, 2 pt in.
const CHEVRON_ZONE_W: f64 = 16.0;
const CHEVRON_D: f64 = 12.0;
const DOT_BOX_D: f64 = 16.0;
/// The colour dot, and the ring the mouse draws around it.
const DOT_D: f64 = 8.0;
const DOT_RING_D: f64 = 12.0;
/// The header's `+` button, its hover ground, and the padding that keeps it
/// flush with the tiles' content column.
const ADD_D: f64 = 24.0;
const ADD_RADIUS: f64 = 8.0;
const ADD_INSET: f64 = 8.0;
/// Feather icons: the box of a button icon, of a link icon, of the pill's.
const ICON_D: f64 = 16.0;
const LINK_ICON_D: f64 = 11.0;
const LINK_ICON_GAP: f64 = 4.0;
const PILL_ICON_D: f64 = 11.0;
/// The view switcher, at the right end of the top row: four 26 pt icon
/// buttons, the selected one on a `SEL_BG` ground.
const SWITCH_BTN: f64 = 26.0;
const SWITCH_ICON_D: f64 = 15.0;
const SWITCH_GAP: f64 = 2.0;
const SWITCH_RADIUS: f64 = 6.0;
/// The switcher's distance from the sidebar's inner right edge.
const SWITCH_INSET: f64 = 10.0;
/// A count badge on a switcher icon: 15 pt at least, hung on the icon's top
/// right corner (4 in from its right edge, 5 above its top) with a 2 pt ring
/// of the sidebar ground so it reads against the selected ground too.
const SWITCH_BADGE_H: f64 = 15.0;
const SWITCH_BADGE_INSET: f64 = 4.0;
const SWITCH_BADGE_RISE: f64 = 5.0;
const SWITCH_BADGE_RING: f64 = 2.0;
/// The failure dot on the Scheduled icon.
const SWITCH_DOT_D: f64 = 8.0;
/// The meta row's ghost sort button: its ground, the icon, its gap to the
/// label, and the padding either side.
const SORT_H: f64 = 16.0;
const SORT_RADIUS: f64 = 4.0;
const SORT_ICON_D: f64 = 11.0;
const SORT_ICON_GAP: f64 = 4.0;
const SORT_PAD: f64 = 4.0;
/// The sort button's padding inside the list content inset.
const META_PAD: f64 = 6.0;
/// A pane tile's vertical padding: 4 above the title, 4 under the subtitle.
const TILE_PAD_V: f64 = 4.0;
/// A pane tile: the row ground's radius and its horizontal padding. The
/// header and the tiles share `ROW_INSET`; this one is the padding inside a
/// tile, from its ground to the dot's box and to the closing column.
const TILE_RADIUS: f64 = 6.0;
const TILE_PAD_H: f64 = 8.0;
/// The one line of a tile: the session name.
const LINE_TITLE_H: f64 = 18.0;
/// A tile's height: 4 + the 18 pt name line + 4, the same whatever the pane
/// holds, so every row of a group is the same size.
const TILE_HEIGHT: f64 = 2.0 * TILE_PAD_V + LINE_TITLE_H;
/// The gap between a tile's right column and the hover buttons beside it.
const COLUMN_GAP: f64 = 8.0;
/// What a hovered tile keeps for its name: the hover buttons that would eat
/// into it are left out (the leftmost first), so hovering never hides it.
const TITLE_MIN_W: f64 = 72.0;
/// The state glyph's box (the 8 pt dot centred in it) and the text column,
/// which starts on the header's title (`CHEVRON_ZONE_W + 16`).
const GLYPH_BOX_W: f64 = 16.0;
const GLYPH_D: f64 = 8.0;
const TEXT_X: f64 = TILE_PAD_H + GLYPH_BOX_W + 8.0;
/// The selected row's rail, at its left edge: 2 pt wide, 16 pt tall, 4 pt in.
const RAIL_W: f64 = 2.0;
const RAIL_H: f64 = 16.0;
const RAIL_INSET: f64 = 4.0;
/// The hover buttons (22 pt, radius 6) and the unread dot before the meta.
const BUTTON_D: f64 = 22.0;
const BUTTON_GAP: f64 = 2.0;
const BUTTON_RADIUS: f64 = 6.0;
const UNREAD_DOT_D: f64 = 6.0;
const HINT_H: f64 = 20.0;
/// Pixels of travel before a pressed header or tile lifts into a drag.
const DRAG_THRESHOLD: f64 = 3.0;
/// The lifted tile: a touch larger and slightly translucent (KovaLink's
/// ghost: scale 1.02, opacity 0.94).
const GHOST_SCALE: f64 = 1.02;
const GHOST_ALPHA: f64 = 0.94;

// --- the `Needs you` block, above the groups ---
/// The card: a raised ground with a border, its padding, and the gap under
/// it before the groups.
const NEED_RADIUS: f64 = 8.0;
const NEED_PAD_TOP: f64 = 4.0;
const NEED_PAD_BOTTOM: f64 = 6.0;
const NEED_PAD_H: f64 = 4.0;
const NEED_MARGIN_B: f64 = 10.0;
/// The header line: `Needs you` on the left, the counts at the right.
const NEED_HEADER_H: f64 = 20.0;
/// One need: a 28 pt row, 6 pt either side of its content.
const NEED_ROW_H: f64 = 28.0;
const NEED_ROW_RADIUS: f64 = 6.0;
const NEED_ROW_PAD_H: f64 = 6.0;
/// Title to detail, and the room the age keeps at the right.
const NEED_GAP: f64 = 8.0;
/// The `Show all` / `Show less` ghost button.
const NEED_BUTTON_H: f64 = 22.0;
/// Below this list width the detail column is dropped and the title takes
/// the room: three columns do not fit a narrow sidebar.
const NEED_DETAIL_MIN_W: f64 = 280.0;
const NEED_TITLE: &str = "Needs you";
const NEED_ANSWER: &str = "Answer";
/// From the end of `waiting 4m` to the `Answer` link: room for the dot
/// separator and its breathing space.
const ANSWER_GAP: f64 = 12.0;

// ---------------------------------------------------------------
// Text styles
// ---------------------------------------------------------------

/// The design system's type scale, Inter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    ViewTitle,
    Title,
    Secondary,
    Chip,
    /// An 11 pt label on a filled button: the confirmation's `Delete`.
    ChipStrong,
    Question,
    Detail,
    Summary,
    Sort,
    Pill,
    PillBadge,
    Link,
    Hint,
    Number,
    /// The Agents stat tile's 20 pt value: the one size above the scale.
    StatValue,
}

/// Font weight, mapped to `NSFontWeight*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
    Bold,
}

impl Style {
    pub const ALL: [Style; 15] = [
        Style::ViewTitle,
        Style::Title,
        Style::Secondary,
        Style::Chip,
        Style::ChipStrong,
        Style::Question,
        Style::Detail,
        Style::Summary,
        Style::Sort,
        Style::Pill,
        Style::PillBadge,
        Style::Link,
        Style::Hint,
        Style::Number,
        Style::StatValue,
    ];

    pub fn size(self) -> f64 {
        match self {
            Style::ViewTitle => 15.0,
            Style::Title => 13.0,
            Style::Secondary | Style::Question | Style::Detail | Style::Link | Style::Pill => 12.0,
            Style::Chip | Style::ChipStrong | Style::Summary | Style::Hint | Style::Number | Style::Sort => 11.0,
            Style::PillBadge => 10.0,
            Style::StatValue => 20.0,
        }
    }

    pub fn weight(self) -> Weight {
        match self {
            Style::ViewTitle | Style::Pill | Style::ChipStrong | Style::StatValue => Weight::Semibold,
            Style::Title | Style::Chip | Style::Link | Style::Sort => Weight::Medium,
            Style::PillBadge => Weight::Bold,
            _ => Weight::Regular,
        }
    }
}

/// Text measurement, so the layout can right-align chips and links and count
/// the lines a question takes. The view answers with AppKit, tests with a
/// stand-in.
pub trait TextMetrics {
    fn width(&self, text: &str, style: Style) -> f64;
    /// Lines `text` wraps into within `width`, capped at `max`.
    fn lines(&self, text: &str, style: Style, width: f64, max: usize) -> usize;
}

// ---------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect { x, y, w, h }
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn inset(&self, d: f64) -> Rect {
        Rect::new(self.x + d, self.y + d, self.w - 2.0 * d, self.h - 2.0 * d)
    }

    fn cg(&self) -> CGRect {
        CGRect { origin: CGPoint { x: self.x, y: self.y }, size: CGSize { width: self.w, height: self.h } }
    }
}

/// What a row of the list is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Header { group: usize },
    Tile { group: usize, index: usize, pane_id: PaneId, column: usize },
    /// One section header of the Scheduled view, with how many rows it holds
    /// (the header says so, folded or not).
    SchedHeader { group: crate::routines::Group, count: usize },
    /// One routine of the scheduled view, by its index in `model.routines`.
    Routine { index: usize },
    /// One row of the `Needs you` block, by its index in `model.needs`.
    Need { index: usize },
    /// The block's `Show all` / `Show less` ghost button.
    NeedAll,
    /// One of the Agents view's four stat tiles.
    AgentStat(sidebar_agents::AgentSection),
    /// A section header of the Agents view, and how many rows it holds.
    AgentHeader { kind: sidebar_agents::SectionKind, count: usize },
    /// One pane row of the Agents view, by its index in `AgentsVm::panes`.
    AgentPane { index: usize, pane_id: PaneId, need: bool },
    /// One `By tab` row, by its index in `AgentsVm::tabs`.
    AgentTab { index: usize, tab_idx: usize },
    /// One manager of the roster, by its index in `AgentsVm::team`.
    AgentTeam { index: usize, pane_id: Option<PaneId> },
    /// One dedicated session, by its index in `AgentsVm::dedicated`.
    AgentDedicated { index: usize, pane_id: Option<PaneId> },
    /// The `None` row of an empty Agents section.
    AgentEmpty,
}

/// One laid-out row, in list (document) coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub kind: RowKind,
    pub frame: Rect,
    /// Tile: the hover glyph boxes on the title line, right to left.
    pub glyphs: Vec<(TileButton, Rect)>,
    /// Tile: the link buttons, on the subtitle line (`Start Claude`,
    /// `Resume`).
    pub actions: Vec<(TileButton, Rect)>,
    /// Need: the detail column, zero width when the list is too narrow.
    pub detail: Rect,
    /// Tile of a pane with an open ask or permission prompt: the `Answer`
    /// link on its subtitle line.
    pub answer: Option<Rect>,
    /// Scheduled: the row's clickable boxes, by spot (the hover strip, the
    /// body's labelled row, the confirmation), in no particular order.
    pub sched: Vec<(SchedButton, Rect, SchedSpot)>,
    /// Scheduled: the inline delete confirmation's box, when it is open.
    pub sched_confirm: Option<Rect>,
}

/// What a point of the list lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListHit {
    /// The chevron zone of a header: fold or unfold, no tab switch.
    Chevron(usize),
    /// The tab colour's bar in a header, and the colour menu: its click box
    /// is unchanged (16 pt around the bar).
    HeaderDot(usize),
    /// The rest of a header: switch to the tab (or fold the active one).
    Header(usize),
    /// The `+` of a header: add a pane.
    HeaderAdd(usize),
    Tile(PaneId),
    TileButton(PaneId, TileButton),
    /// The rest of a group's panel (its padding, the gaps, the empty
    /// bottom): select the tab.
    Panel(usize),
    /// A row of the Scheduled list: a section header, the row itself, or one
    /// of its buttons.
    Sched(SchedHit),
    /// One row of the `Needs you` block: land on what it names.
    Need(usize),
    /// The block's `Show all` / `Show less` button.
    NeedAll,
    /// The `Answer` link of a waiting tile: the same landing as the pane's
    /// ask row.
    NeedAnswer(PaneId),
    /// A button of the Asks list: an answer, Brief, the pane, a snooze.
    Ask(AsksHit),
    /// A row or button of the Agents list.
    Agent(AgentsHit),
    Empty,
}

/// Whether a press on this target should be armed on mouse down.
///
/// `mouse_up` only activates what `mouse_down` armed, so a target missing
/// here is dead on click while still hovering and hit-testing perfectly — the
/// symptom is "nothing happens", with no error anywhere. Every new clickable
/// row belongs in this list and in its test.
fn arms_press(hit: ListHit) -> bool {
    match hit {
        ListHit::Chevron(_)
        | ListHit::HeaderDot(_)
        | ListHit::HeaderAdd(_)
        | ListHit::TileButton(..)
        | ListHit::Panel(_)
        | ListHit::Sched(_)
        | ListHit::Need(_)
        | ListHit::NeedAll
        | ListHit::NeedAnswer(_)
        | ListHit::Ask(_)
        | ListHit::Agent(_) => true,
        // Headers and tiles arm themselves: they may become a drag instead.
        ListHit::Header(_) | ListHit::Tile(_) | ListHit::Empty => false,
    }
}

impl ListHit {
    pub fn pane(self) -> Option<PaneId> {
        match self {
            ListHit::Tile(id) | ListHit::TileButton(id, _) | ListHit::NeedAnswer(id) => Some(id),
            ListHit::Ask(AsksHit::Open(id)) => Some(id),
            _ => None,
        }
    }

    /// The group whose header row this is, whatever part of it.
    pub fn group(self) -> Option<usize> {
        match self {
            ListHit::Chevron(g) | ListHit::HeaderDot(g) | ListHit::Header(g) | ListHit::HeaderAdd(g) => Some(g),
            _ => None,
        }
    }
}

/// The list's geometry for one model at one width.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListLayout {
    pub width: f64,
    pub rows: Vec<Row>,
    /// One block per group, parallel to `model.groups`: the selected one is
    /// a card, and a click inside it outside a row selects the tab.
    pub groups: Vec<Rect>,
    pub hint_y: Option<f64>,
    pub content_h: f64,
    /// The `Needs you` card at the top of the Panes list, when it has rows.
    pub needs_card: Option<Rect>,
    /// Its header line: the title on the left, the counts on the right.
    pub needs_header: Option<Rect>,
    /// The Asks list's cards, when `view` is `SidebarList::Asks`.
    pub asks: AsksLayout,
    /// The Scheduled list's sections and rows, when `view` is
    /// `SidebarList::Scheduled`.
    pub sched: SchedLayout,
    /// The Agents list's tiles, sections and rows, when `view` is
    /// `SidebarList::Agents`.
    pub agents: AgentsLayout,
}

/// The one line the list draws under its rows, in the empty-state slot: the
/// panes hint, the routines read state, or why a list has nothing to show.
fn hint_text(model: &SidebarModel) -> &'static str {
    match model.view {
        SidebarList::Scheduled if !model.routines_loaded => "reading routines\u{2026}",
        SidebarList::Scheduled => "no routines scheduled",
        SidebarList::Asks => "No open questions",
        SidebarList::Agents if model.agents.panes.is_empty() => "No agents in this window",
        SidebarList::Agents => "",
        SidebarList::Panes => "\u{2318}T new tab \u{b7} \u{2318}D split",
    }
}

impl ListLayout {
    pub fn new(model: &SidebarModel, width: f64, m: &dyn TextMetrics) -> Self {
        let x0 = PAD_H;
        let x1 = (width - (PAD_H - EDGE_TOLERANCE)).max(x0 + 40.0);
        let mut rows = Vec::new();
        let mut groups = Vec::with_capacity(model.groups.len());
        let mut y = LIST_TOP;
        if model.view == SidebarList::Asks {
            // The cards carry their own geometry: padding, wrapped message,
            // the action row's buttons and the folded snoozed section.
            let asks = sidebar_asks::layout(&model.asks, width, m);
            let content_h = asks.content_h;
            return ListLayout { width, asks, content_h, ..Default::default() };
        }
        if model.view == SidebarList::Agents {
            // Tiles, sections and rows all come from `sidebar_agents`: one
            // place lays out and draws the view, so the two agree.
            let (agents, rows) = sidebar_agents::layout(&model.agents, &model.agent_state, width, m);
            let hint_y = agents.hint_y;
            let content_h = agents.content_h;
            return ListLayout { width, rows, agents, hint_y, content_h, ..Default::default() };
        }
        if model.view == SidebarList::Scheduled {
            // Sections, rows and their buttons all come from `sidebar_sched`:
            // one place lays out and draws the view, so the two agree.
            let (sched, rows) = sidebar_sched::layout(&model.routines, &model.sched, width, m);
            let hint_y = sched.hint_y;
            let content_h = sched.content_h;
            return ListLayout { width, rows, sched, hint_y, content_h, ..Default::default() };
        }
        // The `Needs you` block: the first thing in the list, above the
        // groups, and it scrolls with them.
        let mut needs_card = None;
        let mut needs_header = None;
        if model.view == SidebarList::Panes && !model.needs.is_empty() {
            let card_top = y;
            let inner_x = x0 + NEED_PAD_H;
            let inner_w = (x1 - x0) - 2.0 * NEED_PAD_H;
            y += NEED_PAD_TOP;
            let header = Rect::new(inner_x, y, inner_w, NEED_HEADER_H);
            y += NEED_HEADER_H;
            let shown = need_rows_shown(&model.needs, model.needs_all);
            for (i, need) in model.needs.iter().enumerate().take(shown) {
                rows.push(need_row(i, need, inner_x, y, inner_w, width, m));
                y += NEED_ROW_H;
            }
            // The button stays once anything would be folded away: it is the
            // only way back down from `Show all`.
            if need_rows_shown(&model.needs, false) < model.needs.len() {
                rows.push(Row {
                    kind: RowKind::NeedAll,
                    frame: Rect::new(inner_x, y, inner_w, NEED_BUTTON_H),
                    glyphs: Vec::new(),
                    actions: Vec::new(),
                    detail: Rect::default(),
                    answer: None,
                    sched: Vec::new(),
                    sched_confirm: None,
                });
                y += NEED_BUTTON_H;
            }
            y += NEED_PAD_BOTTOM;
            needs_card = Some(Rect::new(x0, card_top, x1 - x0, y - card_top));
            needs_header = Some(header);
            y += NEED_MARGIN_B;
        }
        // Every row of a group, header included, sits on the same inset, so
        // their grounds line up under the mouse and when selected; the card
        // itself runs from `x0` to `x1`.
        let row_x = x0 + ROW_INSET;
        let row_w = (x1 - x0) - 2.0 * ROW_INSET;
        for (gi, g) in model.groups.iter().enumerate() {
            if gi > 0 {
                y += GROUP_GAP;
            }
            let top = y;
            // No card around the group unless it is selected: the rows run
            // `GROUP_PAD_V` under the card's top edge, `ROW_GAP` apart.
            y += GROUP_PAD_V;
            rows.push(Row {
                kind: RowKind::Header { group: gi },
                frame: Rect::new(row_x, y, row_w, HEADER_H),
                glyphs: Vec::new(),
                actions: Vec::new(),
                detail: Rect::default(),
                answer: None,
                sched: Vec::new(),
                sched_confirm: None,
            });
            y += HEADER_H;
            for (ti, tile) in g.tiles.iter().enumerate() {
                y += ROW_GAP;
                let row = tile_row(gi, ti, tile, row_x, y, row_w, m);
                y += row.frame.h;
                rows.push(row);
            }
            y += GROUP_PAD_V;
            groups.push(Rect::new(x0, top, x1 - x0, y - top));
        }
        let hint_y = model.show_hint.then(|| {
            if !model.groups.is_empty() {
                y += GROUP_GAP;
            }
            let hy = y;
            y += HINT_H;
            hy
        });
        y += LIST_BOTTOM;
        ListLayout {
            width,
            rows,
            groups,
            hint_y,
            content_h: y,
            needs_card,
            needs_header,
            asks: AsksLayout::default(),
            sched: SchedLayout::default(),
            agents: AgentsLayout::default(),
        }
    }

    /// The `+` button of a header row, flush with the tiles' closing column.
    pub fn add_button(frame: &Rect) -> Rect {
        Rect::new(frame.right() - ADD_INSET - ADD_D, frame.y + (HEADER_H - ADD_D) / 2.0, ADD_D, ADD_D)
    }

    /// The colour menu's box in a header row: 16 pt, what the round dot's
    /// target used to be.
    pub fn dot_button(frame: &Rect) -> Rect {
        let (cx, cy) = Self::dot_centre(frame);
        Rect::new(cx - DOT_BOX_D / 2.0, cy - DOT_BOX_D / 2.0, DOT_BOX_D, DOT_BOX_D)
    }

    /// The centre of the header's tab colour.
    pub fn dot_centre(frame: &Rect) -> (f64, f64) {
        (frame.x + CHEVRON_ZONE_W + 6.0, frame.y + frame.h / 2.0)
    }

    /// The header's colour dot: a filled circle of the tab's colour, centred
    /// in the colour menu's box (`DOT_D` wide).
    pub fn dot_rect(frame: &Rect) -> Rect {
        let (cx, cy) = Self::dot_centre(frame);
        Rect::new(cx - DOT_D / 2.0, cy - DOT_D / 2.0, DOT_D, DOT_D)
    }

    /// The chevron's 12 pt box, centred in the chevron zone.
    pub fn header_chevron(frame: &Rect) -> Rect {
        Rect::new(frame.x + (CHEVRON_ZONE_W - CHEVRON_D) / 2.0, frame.y + (frame.h - CHEVRON_D) / 2.0, CHEVRON_D, CHEVRON_D)
    }

    pub fn hit(&self, x: f64, y: f64) -> ListHit {
        if self.asks.shown {
            return self.asks.hit(x, y).map_or(ListHit::Empty, ListHit::Ask);
        }
        for row in &self.rows {
            if !row.frame.contains(x, y) {
                continue;
            }
            return match row.kind {
                // A scheduled row: its own buttons first (the hover strip,
                // the body's row, the confirmation), then the row itself,
                // which opens its body, or its section header.
                RowKind::SchedHeader { .. } | RowKind::Routine { .. } => {
                    sidebar_sched::hit(row, x, y).map_or(ListHit::Empty, ListHit::Sched)
                }
                // A need row lands on what it names: the pane, the routine,
                // the ask card.
                RowKind::Need { index } => ListHit::Need(index),
                RowKind::NeedAll => ListHit::NeedAll,
                // The Agents list's rows carry their own buttons (the
                // `Open` of a closed roster entry) and their own landing.
                RowKind::AgentStat(_)
                | RowKind::AgentHeader { .. }
                | RowKind::AgentPane { .. }
                | RowKind::AgentTab { .. }
                | RowKind::AgentTeam { .. }
                | RowKind::AgentDedicated { .. }
                | RowKind::AgentEmpty => {
                    sidebar_agents::hit(row, x, y, &self.agents.open_buttons).map_or(ListHit::Empty, ListHit::Agent)
                }
                RowKind::Header { group } => {
                    if x < row.frame.x + CHEVRON_ZONE_W {
                        ListHit::Chevron(group)
                    } else if Self::dot_button(&row.frame).contains(x, y) {
                        ListHit::HeaderDot(group)
                    } else if Self::add_button(&row.frame).contains(x, y) {
                        ListHit::HeaderAdd(group)
                    } else {
                        ListHit::Header(group)
                    }
                }
                RowKind::Tile { pane_id, .. } => {
                    for (button, rect) in row.glyphs.iter().chain(row.actions.iter()) {
                        if rect.contains(x, y) {
                            return ListHit::TileButton(pane_id, *button);
                        }
                    }
                    if row.answer.is_some_and(|rect| rect.contains(x, y)) {
                        return ListHit::NeedAnswer(pane_id);
                    }
                    ListHit::Tile(pane_id)
                }
            };
        }
        // Not on a row: the panel itself, if the point is inside one.
        match self.groups.iter().position(|block| block.contains(x, y)) {
            Some(g) => ListHit::Panel(g),
            None => ListHit::Empty,
        }
    }

    /// The group whose panel `hit` is on, whatever part of it (a tile's
    /// group through its row).
    pub fn group_of(&self, hit: ListHit) -> Option<usize> {
        match hit {
            ListHit::Empty => None,
            ListHit::Panel(g) => Some(g),
            ListHit::Tile(id) | ListHit::TileButton(id, _) => self.row_for_pane(id).and_then(|r| match self.rows[r].kind {
                RowKind::Tile { group, .. } => Some(group),
                _ => None,
            }),
            other => other.group(),
        }
    }

    pub fn row_for_pane(&self, pane_id: PaneId) -> Option<usize> {
        self.rows.iter().position(|r| matches!(r.kind, RowKind::Tile { pane_id: p, .. } if p == pane_id))
    }

    pub fn row_for_group(&self, group: usize) -> Option<usize> {
        self.rows.iter().position(|r| matches!(r.kind, RowKind::Header { group: g } if g == group))
    }
    /// The name line of a pane's tile: the tooltip zone that carries the
    /// folder and the agent the row no longer prints.
    pub fn pane_name_rect(&self, pane_id: PaneId) -> Option<Rect> {
        let row = &self.rows[self.row_for_pane(pane_id)?];
        let x = row.frame.x + TEXT_X;
        Some(Rect::new(x, row.frame.y + TILE_PAD_V, (row.frame.right() - TILE_PAD_H - x).max(0.0), LINE_TITLE_H))
    }

    // --- tab drag ---

    /// Where a dragged group would land: the position (0 ..= groups) such
    /// that the cursor is above the vertical centre of the header at that
    /// position and below the one before it.
    pub fn insertion_index(&self, y: f64) -> usize {
        let mut k = 0;
        for row in &self.rows {
            if let RowKind::Header { .. } = row.kind {
                if y < row.frame.y + row.frame.h / 2.0 {
                    return k;
                }
                k += 1;
            }
        }
        k
    }

    /// Y of the insertion line for position `k`: the gap above the k-th
    /// group, or below the last one.
    pub fn insertion_line_y(&self, k: usize) -> f64 {
        match self.groups.get(k) {
            Some(g) => g.y - GROUP_GAP / 2.0,
            None => self.groups.last().map_or(LIST_TOP, |g| g.bottom() + GROUP_GAP / 2.0),
        }
    }

    // --- pane drag ---

    /// The contiguous run of rows (`start..end`) listing the panes of the
    /// same column of the same group as row `idx`: the only slots a dragged
    /// tile can take.
    pub fn pane_run(&self, idx: usize) -> std::ops::Range<usize> {
        let Some(RowKind::Tile { group, column, .. }) = self.rows.get(idx).map(|r| r.kind) else {
            return idx..idx;
        };
        let same = |r: &Row| matches!(r.kind, RowKind::Tile { group: g, column: c, .. } if g == group && c == column);
        let mut start = idx;
        while start > 0 && same(&self.rows[start - 1]) {
            start -= 1;
        }
        let mut end = idx + 1;
        while end < self.rows.len() && same(&self.rows[end]) {
            end += 1;
        }
        start..end
    }

    /// The rows of `run` as drag slots (position and height).
    pub fn pane_slots(&self, run: &std::ops::Range<usize>) -> Vec<Slot> {
        self.rows.get(run.clone()).map_or_else(Vec::new, |rows| rows.iter().map(|r| Slot { y: r.frame.y, h: r.frame.h }).collect())
    }

    /// The state of a pane drag against this layout: the held row, its run,
    /// the clamped travel, how far each row of the run steps aside and where
    /// the skeleton sits. `None` once the pane is gone.
    pub fn pane_drag_frame(&self, pane_id: PaneId, dy: f64, to: usize) -> Option<PaneDragFrame> {
        let row = self.row_for_pane(pane_id)?;
        let run = self.pane_run(row);
        let slots = self.pane_slots(&run);
        let from = row - run.start;
        let range = (0, slots.len().checked_sub(1)?);
        let (min_dy, max_dy) = drag_bounds(&slots, from, range);
        let dy = dy.clamp(min_dy, max_dy);
        let to = to.clamp(range.0, range.1);
        Some(PaneDragFrame {
            run: run.clone(),
            from,
            to,
            dy,
            moves: displacements(&slots, from, to, ROW_GAP),
            placeholder_y: placeholder_y(&slots, from, to, ROW_GAP),
            slots,
        })
    }
}

/// One frame of a pane drag (`ListLayout::pane_drag_frame`).
#[derive(Clone, Debug, PartialEq)]
pub struct PaneDragFrame {
    /// The rows of the held pane's column run.
    pub run: std::ops::Range<usize>,
    /// The held pane's rank in the run, and the rank it aims at.
    pub from: usize,
    pub to: usize,
    /// The held tile's travel from its row, clamped to the run.
    pub dy: f64,
    pub slots: Vec<Slot>,
    /// How far each row of the run steps aside (the held one stays 0).
    pub moves: Vec<f64>,
    /// The top of the skeleton, in list coordinates.
    pub placeholder_y: f64,
}

impl PaneDragFrame {
    /// The rank the held tile aims at after a move to `dy`, with the run's
    /// hysteresis.
    pub fn next_to(&self, dy: f64) -> usize {
        let range = (0, self.slots.len().saturating_sub(1));
        next_slot(&self.slots, self.from, self.to, dy, ROW_GAP, range)
    }
}

/// A tile's right column: what the row says beside its name. At most one of
/// the three, the first that applies.
struct TileColumn {
    /// Its left edge, and the room it takes from it to the row's content edge.
    x: f64,
    w: f64,
    /// A waiting pane: `waiting 4m`, then its `Answer` link.
    wait: Option<(String, Rect)>,
    answer: Option<Rect>,
    /// The call to action: `Resume` / `Start Claude`.
    link: Option<(TileButton, &'static str, Rect)>,
    /// The state word of a pane that works.
    meta: Option<(String, Rect)>,
}

/// The call to action a tile offers, if any. A waiting pane has none: its
/// right column ends on its `Answer` link.
fn tile_call(tile: &TileVm) -> Option<(TileButton, &'static str)> {
    if tile.waiting.is_some() {
        None
    } else if tile.bare_shell {
        Some((TileButton::StartClaude, START_CLAUDE_LABEL))
    } else if tile.resumable {
        Some((TileButton::Resume, RESUME_LABEL))
    } else {
        None
    }
}

/// Lay out a tile's right column, right aligned at `right` on the line at
/// `y`: the wait, else the call to action, else the state word.
fn tile_column(tile: &TileVm, right: f64, y: f64, h: f64, m: &dyn TextMetrics) -> TileColumn {
    let mut col = TileColumn { x: right, w: 0.0, wait: None, answer: None, link: None, meta: None };
    if let Some(age) = &tile.waiting {
        // `waiting 4m · Answer`: the age in warn, the link after it, its hit
        // rect exactly where the word is drawn.
        let answer_w = m.width(NEED_ANSWER, Style::Link);
        let answer = Rect::new(right - answer_w, y, answer_w, h);
        let label = format!("waiting {age}");
        let wait_w = m.width(&label, Style::Secondary);
        col.x = answer.x - ANSWER_GAP - wait_w;
        col.wait = Some((label, Rect::new(col.x, y, wait_w, h)));
        col.answer = Some(answer);
    } else if let Some((button, label)) = tile_call(tile) {
        let w = link_w(m, label);
        col.x = right - w;
        col.link = Some((button, label, Rect::new(col.x, y, w, h)));
    } else if let Some(text) = tile.meta() {
        let w = m.width(&text, Style::Number);
        col.x = right - w;
        col.meta = Some((text, Rect::new(col.x, y, w, h)));
    }
    col.w = right - col.x;
    col
}

impl TileColumn {
    /// Where the column starts while the row is hovered: a wait shows only
    /// its `Answer` link (the age gives way to the buttons and the name).
    fn hover_x(&self) -> f64 {
        self.answer.map_or(self.x, |a| a.x)
    }
}

/// The hover buttons that fit right of a name kept at `TITLE_MIN_W` or more:
/// `text_x` is where the name starts, `glyph_right` where the buttons end.
fn hover_glyph_count(text_x: f64, glyph_right: f64, wanted: usize) -> usize {
    let room = glyph_right - text_x - TITLE_MIN_W + BUTTON_GAP;
    if room <= 0.0 {
        return 0;
    }
    ((room / (BUTTON_D + BUTTON_GAP)).floor() as usize).min(wanted)
}

/// Lay out one tile at `y`.
fn tile_row(group: usize, index: usize, tile: &TileVm, x: f64, y: f64, w: f64, m: &dyn TextMetrics) -> Row {
    let right = x + w - TILE_PAD_H;
    let h = TILE_HEIGHT;
    let line = Rect::new(x + TEXT_X, y + TILE_PAD_V, (right - (x + TEXT_X)).max(0.0), LINE_TITLE_H);
    let col = tile_column(tile, right, line.y, LINE_TITLE_H, m);
    // The hover buttons sit left of the column, so a call to action or an
    // `Answer` link stays clickable while the mouse is on the row.
    let glyph_right = if col.w > 0.0 { col.hover_x() - COLUMN_GAP } else { right };
    let wanted = TileButton::hover_glyphs(tile.state, tile.minimized, tile.unread);
    let fit = hover_glyph_count(x + TEXT_X, glyph_right, wanted.len());
    let glyphs = wanted
        .into_iter()
        .take(fit)
        .enumerate()
        .map(|(k, b)| {
            let bx = glyph_right - (k as f64 + 1.0) * BUTTON_D - k as f64 * BUTTON_GAP;
            (b, Rect::new(bx, y + TILE_PAD_V + (LINE_TITLE_H - BUTTON_D) / 2.0, BUTTON_D, BUTTON_D))
        })
        .collect();
    let actions: Vec<(TileButton, Rect)> = col.link.iter().map(|(b, _, r)| (*b, *r)).collect();
    Row {
        kind: RowKind::Tile { group, index, pane_id: tile.pane_id, column: tile.column },
        frame: Rect::new(x, y, w, h),
        glyphs,
        actions,
        detail: Rect::default(),
        answer: col.answer,
        sched: Vec::new(),
        sched_confirm: None,
    }
}

/// Lay out one row of the `Needs you` block: the state dot, the title, the
/// detail (dropped below `NEED_DETAIL_MIN_W`) and the age at the right.
fn need_row(index: usize, need: &NeedRowVm, x: f64, y: f64, w: f64, list_w: f64, m: &dyn TextMetrics) -> Row {
    let text_x = x + NEED_ROW_PAD_H + GLYPH_BOX_W;
    let right = x + w - NEED_ROW_PAD_H;
    let age_w = m.width(&need.age, Style::Number);
    let detail = if list_w >= NEED_DETAIL_MIN_W && !need.detail.is_empty() {
        let text_right = right - age_w - NEED_GAP;
        let room = (text_right - text_x).max(0.0);
        // The title never takes more than half: the detail is why the row is
        // there (the command, the error, the question).
        let title_w = m.width(&need.title, Style::Title).min(room * 0.5);
        let dx = text_x + title_w + NEED_GAP;
        Rect::new(dx, y, (text_right - dx).max(0.0), NEED_ROW_H)
    } else {
        Rect::default()
    };
    Row {
        kind: RowKind::Need { index },
        frame: Rect::new(x, y, w, NEED_ROW_H),
        glyphs: Vec::new(),
        actions: Vec::new(),
        detail,
        answer: None,
        sched: Vec::new(),
        sched_confirm: None,
    }
}

/// One run of the block's header counts, so each is coloured apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NeedTally {
    /// Asks and permissions: an agent is blocked on an answer.
    You,
    /// Errors and failed runs: something ended badly.
    Problem,
    /// The ` · ` between two counts.
    Dot,
}

/// `2 need you · 1 error · 1 failed`, zero counts left out.
fn needs_summary(needs: &[NeedRowVm]) -> Vec<(String, NeedTally)> {
    let count = |kinds: &[NeedKind]| needs.iter().filter(|row| kinds.contains(&row.kind)).count();
    let you = count(&[NeedKind::Ask, NeedKind::Permission, NeedKind::Attention]);
    let errors = count(&[NeedKind::Error]);
    let failed = count(&[NeedKind::FailedRoutine]);
    let mut runs = Vec::new();
    let mut push = |text: String, run: NeedTally| {
        if !runs.is_empty() {
            runs.push((" \u{b7} ".to_string(), NeedTally::Dot));
        }
        runs.push((text, run));
    };
    if you > 0 {
        push(format!("{you} need you"), NeedTally::You);
    }
    if errors > 0 {
        push(format!("{errors} error{}", if errors == 1 { "" } else { "s" }), NeedTally::Problem);
    }
    if failed > 0 {
        push(format!("{failed} failed"), NeedTally::Problem);
    }
    runs
}

/// The block's ghost button: `Show all N` while it folds rows away, `Show
/// less` once everything is shown.
fn need_all_label(total: usize, all: bool) -> String {
    if all { "Show less".to_string() } else { format!("Show all {total}") }
}

/// The box of a link (`Resume`, `Stop`): its icon, the gap, the word, and
/// 4 pt either side.
fn link_w(m: &dyn TextMetrics, label: &str) -> f64 {
    LINK_ICON_D + LINK_ICON_GAP + m.width(label, Style::Link) + 8.0
}

/// What a point of the chrome lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeHit {
    /// The traffic-light strip: window drag region.
    TopArea,
    SortToggle,
    NextPill,
    /// One of the four view-switcher buttons, at the right end of the top row.
    ViewSwitch(SidebarList),
    /// The resize handle at the right edge.
    Edge,
    Empty,
}

/// The fixed parts of the sidebar, in the sidebar view's flipped coordinates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChromeLayout {
    pub width: f64,
    pub height: f64,
    /// The list's name, on the title row's left. Its width stops short of
    /// the pill, so a long name truncates before it collides with it.
    pub title: Rect,
    pub summary: Rect,
    pub sort: Rect,
    /// The Next pill: sized to its content, against the right edge. Also
    /// its hit region.
    pub pill: Rect,
    /// The four view-switcher buttons on the top row, left to right.
    pub switch: [Rect; 4],
    pub list: Rect,
}

/// The list each switcher button selects, in the order they are drawn.
pub const SWITCH_LISTS: [SidebarList; 4] =
    [SidebarList::Panes, SidebarList::Asks, SidebarList::Agents, SidebarList::Scheduled];

/// The width of the Next pill around its content: the side padding, the
/// filled play and its gap when the pill is clickable, the label, and the
/// count badge with its gap when there is a count.
pub fn pill_width(pill: NextPill, m: &dyn TextMetrics) -> f64 {
    let icon = if pill.clickable() { PILL_ICON_D + PILL_ICON_GAP } else { 0.0 };
    let badge = pill.badge().map_or(0.0, |n| PILL_BADGE_GAP + badge_width(n, m));
    2.0 * PILL_PAD + icon + m.width(pill.label(), Style::Pill) + badge
}

/// The count badge: a circle at least, wider for two digits and more.
fn badge_width(n: usize, m: &dyn TextMetrics) -> f64 {
    (m.width(&n.to_string(), Style::PillBadge) + 8.0).max(PILL_BADGE_H)
}

/// A badge hung on a switcher icon's top right corner: 4 pt in from its
/// right edge, 5 pt above its top.
fn switch_badge(btn: &Rect, w: f64, h: f64) -> Rect {
    Rect::new(btn.right() - SWITCH_BADGE_INSET - w, btn.y - SWITCH_BADGE_RISE, w, h)
}

impl ChromeLayout {
    /// `sort_label_w` is `None` on a list that has no sort control: the
    /// ghost and its hit target are left out.
    pub fn new(width: f64, height: f64, sort_label_w: Option<f64>, pill_w: f64) -> Self {
        let inner_w = width - SEP_W;
        // Right-aligned on the traffic-light row; the pill sits on the row
        // below, so the two never compete for the width.
        let switch_w = SWITCH_LISTS.len() as f64 * SWITCH_BTN + (SWITCH_LISTS.len() - 1) as f64 * SWITCH_GAP;
        let switch_x = (inner_w - SWITCH_INSET - switch_w).max(0.0);
        let switch_y = (TOP_H - SWITCH_BTN) / 2.0;
        let switch =
            std::array::from_fn(|i| Rect::new(switch_x + i as f64 * (SWITCH_BTN + SWITCH_GAP), switch_y, SWITCH_BTN, SWITCH_BTN));
        // The title keeps its gap before the pill whatever the width: the
        // pill gives way first, down to nothing on a narrow sidebar.
        let pill_w = pill_w.min(inner_w - PILL_INSET - TITLE_LEFT - TITLE_GAP).max(0.0);
        let pill = Rect::new(inner_w - PILL_INSET - pill_w, TOP_H + (TITLE_H - PILL_H) / 2.0, pill_w, PILL_H);
        let title = Rect::new(TITLE_LEFT, TOP_H, (pill.x - TITLE_GAP - TITLE_LEFT).max(0.0), TITLE_H);
        // The meta row sits inside the list's own 12 pt inset, 6 more in on
        // either side, so its text lines up with the tiles' content.
        let meta_x = PAD_H + META_PAD;
        let summary = Rect::new(meta_x, TOP_H + TITLE_H, inner_w - 2.0 * meta_x, META_H);
        let sort = sort_label_w.map_or_else(Rect::default, |label_w| {
            let sort_w = label_w + SORT_ICON_D + SORT_ICON_GAP + 2.0 * SORT_PAD;
            Rect::new(summary.right() - sort_w, summary.y + (META_H - SORT_H) / 2.0, sort_w, SORT_H)
        });
        let list_y = TOP_H + TITLE_H + META_H;
        let list = Rect::new(0.0, list_y, inner_w - EDGE_TOLERANCE, (height - list_y).max(0.0));
        ChromeLayout { width, height, title, summary, sort, pill, switch, list }
    }

    pub fn hit(&self, x: f64, y: f64) -> ChromeHit {
        if (x - (self.width - SEP_W / 2.0)).abs() <= EDGE_TOLERANCE {
            return ChromeHit::Edge;
        }
        if x < 0.0 || x >= self.width || y < 0.0 || y >= self.height {
            return ChromeHit::Empty;
        }
        // The buttons come before the top-area fallback: they live inside
        // it, and a press on one must switch lists, not drag the window.
        for (rect, list) in self.switch.iter().zip(SWITCH_LISTS) {
            if rect.contains(x, y) {
                return ChromeHit::ViewSwitch(list);
            }
        }
        if y < TOP_H {
            return ChromeHit::TopArea;
        }
        if self.sort.w > 0.0 && self.sort.contains(x, y) {
            return ChromeHit::SortToggle;
        }
        if self.pill.contains(x, y) {
            return ChromeHit::NextPill;
        }
        ChromeHit::Empty
    }
}

// ---------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------

/// A drag is anchored to the tab's id, not its group index: the tick
/// re-lays the list out while the mouse is down, and the groups move.
#[derive(Clone, Copy)]
struct TabDrag {
    tab_id: TabId,
    start_y: f64,
    current_y: f64,
    /// Where inside the header the cursor grabbed it, so the floating copy
    /// keeps that offset.
    grab_offset: f64,
    dragging: bool,
}

/// Same for a tile: the pane id, resolved to a row when drawn or dropped.
/// `to` is the rank the tile aims at inside its column run (KovaLink's
/// drag machine: the other rows step aside and a skeleton marks it).
#[derive(Clone, Copy)]
struct PaneDrag {
    pane_id: PaneId,
    start_y: f64,
    current_y: f64,
    dragging: bool,
    to: usize,
}

pub(super) struct Shared {
    model: SidebarModel,
    layout: ListLayout,
    chrome: ChromeLayout,
    fonts: Vec<(Style, Retained<NSFont>)>,
    hovered: ListHit,
    /// The tab whose panel the mouse is on (any part of it): its wash
    /// brightens. The id, not the group index: a tick may reorder groups.
    hovered_tab: Option<TabId>,
    /// What the mouse went down on; the action fires on mouse up inside the
    /// same target, like a button.
    pressed: ListHit,
    chrome_hovered: ChromeHit,
    chrome_pressed: ChromeHit,
    tab_drag: Option<TabDrag>,
    pane_drag: Option<PaneDrag>,
    edge_drag: bool,
}

impl Shared {
    fn font(&self, style: Style) -> &NSFont {
        &self.fonts.iter().find(|(s, _)| *s == style).expect("every style has a font").1
    }
}

/// Text measurement through AppKit.
pub(super) struct AppKitMetrics<'a>(pub(super) &'a Shared);

impl TextMetrics for AppKitMetrics<'_> {
    fn width(&self, text: &str, style: Style) -> f64 {
        let attrs = attrs(self.0.font(style), tokens::get().text_primary, 1.0, NSLineBreakMode::ByClipping);
        let size = unsafe { NSString::from_str(text).sizeWithAttributes(Some(&attrs)) };
        size.width.ceil()
    }

    fn lines(&self, text: &str, style: Style, width: f64, max: usize) -> usize {
        if text.is_empty() {
            return 1;
        }
        let attrs = attrs(self.0.font(style), tokens::get().text_primary, 1.0, NSLineBreakMode::ByWordWrapping);
        let one = unsafe { NSString::from_str("X").sizeWithAttributes(Some(&attrs)) }.height.max(1.0);
        let rect = unsafe {
            NSString::from_str(text).boundingRectWithSize_options_attributes_context(
                CGSize { width, height: 10_000.0 },
                NSStringDrawingOptions::UsesLineFragmentOrigin,
                Some(&attrs),
                None,
            )
        };
        ((rect.size.height / one).round() as usize).clamp(1, max)
    }
}

// ---------------------------------------------------------------
// Drawing helpers
// ---------------------------------------------------------------

fn color(c: [f32; 3], alpha: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(c[0] as f64, c[1] as f64, c[2] as f64, alpha)
}

pub(super) fn fill_round(r: &Rect, radius: f64, c: [f32; 3], alpha: f64) {
    color(c, alpha).setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r.cg(), radius, radius).fill();
}

pub(super) fn stroke_round(r: &Rect, radius: f64, width: f64, c: [f32; 3], alpha: f64) {
    color(c, alpha).setStroke();
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r.inset(width / 2.0).cg(), radius, radius);
    path.setLineWidth(width);
    path.stroke();
}

fn fill_rect(r: &Rect, c: [f32; 3], alpha: f64) {
    color(c, alpha).setFill();
    NSBezierPath::bezierPathWithRect(r.cg()).fill();
}

pub(super) fn fill_dot(cx: f64, cy: f64, d: f64, c: [f32; 3], alpha: f64) {
    color(c, alpha).setFill();
    NSBezierPath::bezierPathWithOvalInRect(Rect::new(cx - d / 2.0, cy - d / 2.0, d, d).cg()).fill();
}

pub(super) fn stroke_ring(cx: f64, cy: f64, d: f64, width: f64, c: [f32; 3], alpha: f64) {
    color(c, alpha).setStroke();
    let path = NSBezierPath::bezierPathWithOvalInRect(Rect::new(cx - d / 2.0, cy - d / 2.0, d, d).inset(width / 2.0).cg());
    path.setLineWidth(width);
    path.stroke();
}

/// A Feather icon stroked in a `size` box centred in `r`.
pub(super) fn stroke_icon(icon: Icon, r: &Rect, size: f64, c: [f32; 3], alpha: f64) {
    color(c, alpha).setStroke();
    feather::path(icon, feather::centred_box((r.x, r.y, r.w, r.h), size)).stroke();
}

/// A Feather icon filled (the solid play and square) in a `size` box
/// centred in `r`.
fn fill_icon(icon: Icon, r: &Rect, size: f64, c: [f32; 3], alpha: f64) {
    color(c, alpha).setFill();
    feather::path(icon, feather::centred_box((r.x, r.y, r.w, r.h), size)).fill();
}

/// A menu swatch: a 12 pt image holding a filled dot in `c`, or a hollow
/// grey ring for "No colour". Drawn through a handler so it stays sharp on
/// Retina.
pub(super) fn swatch_image(c: Option<[f32; 3]>) -> Retained<NSImage> {
    let size = CGSize { width: 12.0, height: 12.0 };
    let handler = block2::RcBlock::new(move |_rect: CGRect| -> objc2::runtime::Bool {
        match c {
            Some(c) => fill_dot(6.0, 6.0, 10.0, c, 1.0),
            None => stroke_ring(6.0, 6.0, 10.0, 1.5, tokens::get().tab_none, 1.0),
        }
        objc2::runtime::Bool::YES
    });
    NSImage::imageWithSize_flipped_drawingHandler(size, false, &handler)
}

/// A rounded fill with a soft shadow under it: 6 pt down, `blur` wide,
/// black at `shadow_alpha`.
fn fill_round_shadowed(r: &Rect, radius: f64, c: [f32; 3], alpha: f64, blur: f64, shadow_alpha: f64) {
    NSGraphicsContext::saveGraphicsState_class();
    let shadow = NSShadow::new();
    // Shadow offsets are in the window's base space, y up: negative is down.
    shadow.setShadowOffset(CGSize { width: 0.0, height: -6.0 });
    shadow.setShadowBlurRadius(blur);
    shadow.setShadowColor(Some(&color(tokens::get().black, shadow_alpha * alpha)));
    shadow.set();
    fill_round(r, radius, c, alpha);
    NSGraphicsContext::restoreGraphicsState_class();
}

/// A dashed rounded outline: the drag skeleton.
fn stroke_dashed_round(r: &Rect, radius: f64, width: f64, c: [f32; 3], alpha: f64) {
    color(c, alpha).setStroke();
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r.inset(width / 2.0).cg(), radius, radius);
    path.setLineWidth(width);
    let pattern: [f64; 2] = [5.0, 4.0];
    unsafe { path.setLineDash_count_phase(pattern.as_ptr(), pattern.len() as isize, 0.0) };
    path.stroke();
}

/// Text attributes: font, colour, truncation.
fn attrs(font: &NSFont, c: [f32; 3], alpha: f64, mode: NSLineBreakMode) -> Retained<NSDictionary<NSString, AnyObject>> {
    let style = NSMutableParagraphStyle::new();
    style.setLineBreakMode(mode);
    let color = color(c, alpha);
    unsafe {
        NSDictionary::from_slices::<NSString>(
            &[
                objc2_app_kit::NSFontAttributeName,
                objc2_app_kit::NSForegroundColorAttributeName,
                objc2_app_kit::NSParagraphStyleAttributeName,
            ],
            &[font as &AnyObject, &*color as &AnyObject, &*style as &AnyObject],
        )
    }
}

/// Horizontal alignment of a one-line text inside its box.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Align {
    Left,
    Right,
    Center,
}

/// Draw one line of text vertically centred in `r`, truncated with an
/// ellipsis (at the tail; at the head for rename edit buffers, so the cursor
/// stays visible). Returns the drawn width.
pub(super) fn draw_text(sh: &Shared, text: &str, r: &Rect, style: Style, c: [f32; 3], alpha: f64, align: Align, head: bool) -> f64 {
    let mode = if head { NSLineBreakMode::ByTruncatingHead } else { NSLineBreakMode::ByTruncatingTail };
    let a = attrs(sh.font(style), c, alpha, mode);
    let ns = NSString::from_str(text);
    let size = unsafe { ns.sizeWithAttributes(Some(&a)) };
    let w = size.width.min(r.w).max(0.0);
    let x = match align {
        Align::Left => r.x,
        Align::Right => r.right() - w,
        Align::Center => r.x + (r.w - w) / 2.0,
    };
    let y = r.y + ((r.h - size.height) / 2.0).round();
    unsafe { ns.drawInRect_withAttributes(Rect::new(x, y, w, size.height).cg(), Some(&a)) };
    w
}

/// Draw a wrapped text of at most `lines` lines in `r`, the last line ending
/// with an ellipsis when cut.
pub(super) fn draw_wrapped(sh: &Shared, text: &str, r: &Rect, style: Style, c: [f32; 3], alpha: f64) {
    let a = attrs(sh.font(style), c, alpha, NSLineBreakMode::ByWordWrapping);
    unsafe {
        NSString::from_str(text).drawWithRect_options_attributes_context(
            r.cg(),
            NSStringDrawingOptions::UsesLineFragmentOrigin | NSStringDrawingOptions::TruncatesLastVisibleLine,
            Some(&a),
            None,
        )
    };
}

/// A link: a filled Feather icon, a gap, the word, the pair centred in `r`.
fn draw_link(sh: &Shared, icon: Icon, label: &str, r: &Rect, c: [f32; 3], alpha: f64) {
    let text_w = AppKitMetrics(sh).width(label, Style::Link).min(r.w - LINK_ICON_D - LINK_ICON_GAP).max(0.0);
    let w = LINK_ICON_D + LINK_ICON_GAP + text_w;
    let x = r.x + (r.w - w) / 2.0;
    fill_icon(icon, &Rect::new(x, r.y, LINK_ICON_D, r.h), LINK_ICON_D, c, alpha);
    draw_text(sh, label, &Rect::new(x + LINK_ICON_D + LINK_ICON_GAP, r.y, text_w, r.h), Style::Link, c, alpha, Align::Left, false);
}

/// The ink of a tile's links: accent on the ground, t1 once the tile is
/// focused (accent on the selected blue is blue on blue) and under the mouse.
fn link_ink(focused: bool, hovered: bool) -> [f32; 3] {
    if focused || hovered { tokens::get().text_primary } else { tokens::get().accent }
}

/// The `KovaView` of the window a sidebar view sits in.
fn kova_of(view: &NSView) -> Option<Retained<KovaView>> {
    let window = view.window()?;
    crate::app::kova_view(&window).map(|v| v.retain())
}

/// The event's location in a flipped view's coordinates.
fn local_point(view: &NSView, event: &NSEvent) -> (f64, f64) {
    let p = view.convertPoint_fromView(event.locationInWindow(), None);
    (p.x, p.y)
}

fn build_fonts() -> Vec<(Style, Retained<NSFont>)> {
    super::fonts::register_inter();
    Style::ALL
        .iter()
        .map(|&s| {
            let size = s.size();
            let font = NSFont::fontWithName_size(&NSString::from_str(super::fonts::inter_name(s.weight())), size)
                .unwrap_or_else(|| {
                    let weight = unsafe {
                        match s.weight() {
                            Weight::Regular => objc2_app_kit::NSFontWeightRegular,
                            Weight::Medium => objc2_app_kit::NSFontWeightMedium,
                            Weight::Semibold => objc2_app_kit::NSFontWeightSemibold,
                            Weight::Bold => objc2_app_kit::NSFontWeightBold,
                        }
                    };
                    NSFont::systemFontOfSize_weight(size, weight)
                });
            (s, font)
        })
        .collect()
}

// ---------------------------------------------------------------
// SidebarView: chrome and scroll view
// ---------------------------------------------------------------

/// The Brief overlay's one native field: the question it edits, the box that
/// carries its ground and border, and the rect the last layout gave it (moved
/// only when that changes, never rebuilt on a tick).
struct BriefField {
    id: String,
    box_: Retained<NSView>,
    field: Retained<NSTextField>,
    rect: Rect,
}

impl BriefField {
    /// The field editor is open on this field: the keys go here, not to the
    /// terminal. `currentEditor` is AppKit's own answer to that question.
    fn current_editor(&self) -> bool {
        self.field.currentEditor().is_some()
    }
}

pub struct SidebarViewIvars {
    shared: Rc<RefCell<Shared>>,
    scroll: OnceCell<Retained<NSScrollView>>,
    list: OnceCell<Retained<SidebarListView>>,
    brief_field: RefCell<Option<BriefField>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "KovaSidebarView"]
    #[ivars = SidebarViewIvars]
    pub struct SidebarView;

    unsafe impl NSObjectProtocol for SidebarView {}

    /// The Brief field's callbacks: the text feeds the draft and grows the
    /// card, Enter starts a new line, Cmd+Enter sends, Escape cancels.
    unsafe impl NSControlTextEditingDelegate for SidebarView {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, _notification: &NSNotification) {
            self.brief_text_changed();
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn control_text_view_do_command(&self, _control: &NSControl, text_view: &NSTextView, selector: Sel) -> bool {
            self.brief_command(text_view, selector)
        }
    }

    unsafe impl NSTextFieldDelegate for SidebarView {}

    impl SidebarView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            false
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }

        /// Cmd+Enter in the Brief field sends it. AppKit routes a key
        /// equivalent before the field editor turns it into a command, so the
        /// delegate never sees this one: this is its door. Return alone stays
        /// a newline (`brief_command`), and the field must be the one being
        /// edited, or the terminal keeps its own keys.
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> objc2::runtime::Bool {
            // Whatever a subview claims stays its own.
            let handled: objc2::runtime::Bool = unsafe { msg_send![super(self), performKeyEquivalent: event] };
            if handled.as_bool() {
                return handled;
            }
            let enter = event.keyCode() == 36 || event.keyCode() == 76;
            if enter
                && event.modifierFlags().contains(NSEventModifierFlags::Command)
                && self.brief_field_editing()
            {
                if let Some(key) = self.brief_key() {
                    log::info!("sidebar: brief send (cmd+enter)");
                    self.activate_brief(AsksHit::BriefSend(key));
                }
                return objc2::runtime::Bool::YES;
            }
            objc2::runtime::Bool::NO
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            true
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, new_size: CGSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: new_size] };
            self.layout_chrome();
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: CGRect) {
            self.draw_chrome();
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            let b = self.bounds();
            let edge = Rect::new(b.size.width - SEP_W - EDGE_TOLERANCE, 0.0, SEP_W + EDGE_TOLERANCE, b.size.height);
            #[allow(deprecated)]
            self.addCursorRect_cursor(edge.cg(), &NSCursor::resizeLeftRightCursor());
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            install_tracking_area(self);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            let (x, y) = local_point(self, event);
            let hit = self.ivars().shared.borrow().chrome.hit(x, y);
            let hovered = match hit {
                ChromeHit::SortToggle | ChromeHit::NextPill | ChromeHit::ViewSwitch(_) => hit,
                _ => ChromeHit::Empty,
            };
            let mut sh = self.ivars().shared.borrow_mut();
            if sh.chrome_hovered != hovered {
                sh.chrome_hovered = hovered;
                drop(sh);
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            let mut sh = self.ivars().shared.borrow_mut();
            if sh.chrome_hovered != ChromeHit::Empty {
                sh.chrome_hovered = ChromeHit::Empty;
                drop(sh);
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            // The wheel over the chrome scrolls the list, like on the phone.
            if let Some(scroll) = self.ivars().scroll.get() {
                scroll.scrollWheel(event);
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let (x, y) = local_point(self, event);
            let hit = self.ivars().shared.borrow().chrome.hit(x, y);
            {
                let mut sh = self.ivars().shared.borrow_mut();
                sh.chrome_pressed = ChromeHit::Empty;
                sh.edge_drag = false;
            }
            match hit {
                ChromeHit::Edge => self.ivars().shared.borrow_mut().edge_drag = true,
                ChromeHit::TopArea => {
                    if let Some(win) = self.window() {
                        if event.clickCount() == 2 {
                            win.zoom(None);
                        } else {
                            win.performWindowDragWithEvent(event);
                        }
                    }
                }
                ChromeHit::SortToggle | ChromeHit::NextPill | ChromeHit::ViewSwitch(_) => {
                    self.ivars().shared.borrow_mut().chrome_pressed = hit;
                    self.setNeedsDisplay(true);
                }
                ChromeHit::Empty => {}
            }
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let (x, y) = local_point(self, event);
            let (edge, pressed) = {
                let sh = self.ivars().shared.borrow();
                (sh.edge_drag, sh.chrome_pressed)
            };
            if edge {
                let pts = (x + SEP_W / 2.0).round().max(0.0) as u16;
                if sidebar::set_width_pt(pts) {
                    if let Some(kova) = kova_of(self) {
                        kova.apply_layout();
                    }
                }
                return;
            }
            if pressed != ChromeHit::Empty {
                let hit = self.ivars().shared.borrow().chrome.hit(x, y);
                if hit != pressed {
                    self.ivars().shared.borrow_mut().chrome_pressed = ChromeHit::Empty;
                    self.setNeedsDisplay(true);
                }
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let (x, y) = local_point(self, event);
            let (edge, pressed) = {
                let mut sh = self.ivars().shared.borrow_mut();
                (std::mem::take(&mut sh.edge_drag), std::mem::replace(&mut sh.chrome_pressed, ChromeHit::Empty))
            };
            if edge {
                sidebar::persist();
                return;
            }
            if pressed != ChromeHit::Empty {
                let hit = self.ivars().shared.borrow().chrome.hit(x, y);
                if hit == pressed {
                    if let Some(kova) = kova_of(self) {
                        match pressed {
                            ChromeHit::SortToggle => kova.sidebar_toggle_sort(),
                            ChromeHit::ViewSwitch(list) => kova.sidebar_show_list(list),
                            ChromeHit::NextPill => kova.sidebar_next_pill_clicked(),
                            _ => {}
                        }
                    }
                }
                self.setNeedsDisplay(true);
            }
        }
    }
);

/// One tracking area over the whole view: hover and exit.
fn install_tracking_area(view: &NSView) {
    let old_areas: Vec<_> = view.trackingAreas().to_vec();
    for area in &old_areas {
        view.removeTrackingArea(area);
    }
    let options = NSTrackingAreaOptions::MouseMoved
        | NSTrackingAreaOptions::MouseEnteredAndExited
        | NSTrackingAreaOptions::ActiveInKeyWindow
        | NSTrackingAreaOptions::InVisibleRect;
    let area = unsafe {
        let alloc: objc2::rc::Allocated<NSTrackingArea> = msg_send![objc2::class!(NSTrackingArea), alloc];
        NSTrackingArea::initWithRect_options_owner_userInfo(alloc, view.bounds(), options, Some(view.as_ref()), None)
    };
    view.addTrackingArea(&area);
}

impl SidebarView {
    /// The brief text field owns the keyboard: it is the field editor's
    /// client right now, not just a field sitting in the view.
    fn brief_field_editing(&self) -> bool {
        self.ivars().brief_field.borrow().as_ref().is_some_and(|open| open.current_editor())
    }

    pub fn new(mtm: MainThreadMarker, frame: CGRect) -> Retained<Self> {
        let shared = Rc::new(RefCell::new(Shared {
            model: SidebarModel {
                view: SidebarList::default(),
                routines: Vec::new(),
                routines_loaded: false,
                asks_waiting: 0,
                routine_failed: false,
                summary: Vec::new(),
                sort: SidebarSort::Kova,
                pill: NextPill::Nothing,
                groups: Vec::new(),
                needs: Vec::new(),
                needs_all: false,
                asks: Default::default(),
                show_hint: false,
                sched: Default::default(),
                sched_meta: Vec::new(),
                agents: Default::default(),
                agent_state: Default::default(),
                meta_right: None,
            },
            layout: ListLayout::default(),
            chrome: ChromeLayout::default(),
            fonts: build_fonts(),
            hovered: ListHit::Empty,
            hovered_tab: None,
            pressed: ListHit::Empty,
            chrome_hovered: ChromeHit::Empty,
            chrome_pressed: ChromeHit::Empty,
            tab_drag: None,
            pane_drag: None,
            edge_drag: false,
        }));
        let this = mtm.alloc::<Self>().set_ivars(SidebarViewIvars {
            shared: shared.clone(),
            scroll: OnceCell::new(),
            list: OnceCell::new(),
            brief_field: RefCell::new(None),
        });
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.setWantsLayer(true);

        let scroll = NSScrollView::initWithFrame(mtm.alloc(), CGRect::ZERO);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setHasHorizontalScroller(false);
        scroll.setAutohidesScrollers(true);
        scroll.setScrollerStyle(NSScrollerStyle::Overlay);
        scroll.setVerticalScrollElasticity(NSScrollElasticity::Allowed);
        scroll.setHorizontalScrollElasticity(NSScrollElasticity::None);
        let list = SidebarListView::new(mtm, shared, CGRect::ZERO);
        scroll.setDocumentView(Some(&list));
        view.addSubview(&scroll);
        view.ivars().scroll.set(scroll).ok();
        view.ivars().list.set(list).ok();
        view.layout_chrome();
        view
    }

    /// Adopt a new model. Returns whether anything changed (the list is
    /// re-laid out and redrawn only then).
    pub fn set_model(&self, model: SidebarModel) -> bool {
        {
            let sh = self.ivars().shared.borrow();
            if sh.model == model {
                return false;
            }
        }
        let chrome_changed = {
            let mut sh = self.ivars().shared.borrow_mut();
            let changed = sh.model.sort != model.sort || sh.model.pill != model.pill || sh.model.view != model.view;
            sh.model = model;
            changed
        };
        // A new sort label or pill content changes the chrome (and, through
        // it, the list); anything else only moves the rows.
        if chrome_changed {
            self.layout_chrome();
        } else {
            self.relayout_list();
            self.setNeedsDisplay(true);
        }
        true
    }

    /// Repaint everything after a change the model does not carry: a
    /// `reload-config` can switch the palette under the same model.
    pub fn repaint(&self) {
        if let Some(list) = self.ivars().list.get() {
            list.setNeedsDisplay(true);
        }
        self.setNeedsDisplay(true);
    }

    /// Scroll the list the least that shows a pane's tile (or, when its
    /// group is folded, the header of tab `tab_idx`).
    pub fn reveal(&self, pane_id: PaneId, tab_idx: usize) {
        let Some(list) = self.ivars().list.get() else { return };
        let frame = {
            let sh = self.ivars().shared.borrow();
            let row = sh.layout.row_for_pane(pane_id).or_else(|| {
                let g = sh.model.groups.iter().position(|g| g.tab_idx == tab_idx)?;
                sh.layout.row_for_group(g)
            });
            row.map(|r| sh.layout.rows[r].frame)
        };
        if let Some(f) = frame {
            list.scrollRectToVisible(Rect::new(f.x, f.y - GROUP_GAP, f.w, f.h + 2.0 * GROUP_GAP).cg());
        }
    }

    /// Scroll the least that shows the Asks card `id`, for a need row that
    /// walked to it.
    pub fn reveal_ask(&self, id: &str) {
        let Some(list) = self.ivars().list.get() else { return };
        let frame = self.ivars().shared.borrow().layout.asks.card_frame(id);
        if let Some(f) = frame {
            list.scrollRectToVisible(Rect::new(f.x, f.y - GROUP_GAP, f.w, f.h + 2.0 * GROUP_GAP).cg());
        }
    }

    /// Position the scroll view for the current size, and re-lay the list.
    fn layout_chrome(&self) {
        let b = self.bounds();
        let (sort_w, pill_w) = {
            let sh = self.ivars().shared.borrow();
            let m = AppKitMetrics(&sh);
            // The sort ghost is the Panes list's own control.
            let sort_w = matches!(sh.model.view, SidebarList::Panes).then(|| m.width(sh.model.sort.label(), Style::Sort));
            (sort_w, pill_width(sh.model.pill, &m))
        };
        let chrome = ChromeLayout::new(b.size.width, b.size.height, sort_w, pill_w);
        let list_rect = chrome.list;
        self.ivars().shared.borrow_mut().chrome = chrome;
        // Rows first, then the scroll view: the model changed already, and
        // nothing of AppKit runs between the two.
        self.relayout_list();
        if let Some(scroll) = self.ivars().scroll.get() {
            scroll.setFrame(list_rect.cg());
        }
        self.setNeedsDisplay(true);
        if let Some(win) = self.window() {
            win.invalidateCursorRectsForView(self);
        }
    }

    /// Lay the rows out at the scroll view's width and size the document.
    fn relayout_list(&self) {
        let Some(list) = self.ivars().list.get() else { return };
        let width = self.ivars().shared.borrow().chrome.list.w.max(60.0);
        let (content_h, layout) = {
            let sh = self.ivars().shared.borrow();
            let layout = ListLayout::new(&sh.model, width, &AppKitMetrics(&sh));
            (layout.content_h, layout)
        };
        {
            let mut sh = self.ivars().shared.borrow_mut();
            sh.layout = layout;
            // The rows moved: whatever was under the mouse may not be any more.
            if sh.hovered != ListHit::Empty && sh.tab_drag.is_none() && sh.pane_drag.is_none() {
                sh.hovered = ListHit::Empty;
                sh.hovered_tab = None;
            }
        }
        let visible_h = self.ivars().shared.borrow().chrome.list.h;
        list.setFrameSize(CGSize { width, height: content_h.max(visible_h) });
        list.install_tooltips();
        // The panels moved: their hand-cursor rects follow.
        if let Some(win) = list.window() {
            win.invalidateCursorRectsForView(&list);
        }
        self.sync_brief_field(&list);
        list.setNeedsDisplay(true);
    }

    // ---------------------------------------------------------------
    // Brief overlay's native field
    // ---------------------------------------------------------------

    /// Make the one native field match the overlay: created and focused when
    /// a Brief opens, moved when its rect changes, removed when the overlay
    /// closes or its question leaves the board. Never rebuilt on a tick.
    fn sync_brief_field(&self, list: &SidebarListView) {
        let target = {
            let sh = self.ivars().shared.borrow();
            match (&sh.model.asks.brief, &sh.layout.asks.brief) {
                (Some(brief), Some(layout)) => Some((
                    brief.id.clone(),
                    sh.model.asks.drafts.get(&brief.id).cloned().unwrap_or_default(),
                    layout.card,
                    layout.field,
                )),
                _ => None,
            }
        };
        let Some((id, text, card, rect)) = target else {
            self.close_brief_field();
            return;
        };
        let open = self.ivars().brief_field.borrow().as_ref().is_some_and(|open| open.id == id);
        if open {
            let mut slot = self.ivars().brief_field.borrow_mut();
            let field = slot.as_mut().expect("just checked");
            if field.rect != rect {
                field.rect = rect;
                field.box_.setFrame(rect.cg());
                Self::place_brief_text(field);
            }
            return;
        }
        self.close_brief_field();
        self.open_brief_field(list, &id, &text, card, rect);
    }

    /// The text area inside the box: the pads the pure layout measured with.
    fn place_brief_text(brief: &BriefField) {
        let inner = Rect::new(
            sidebar_asks::BRIEF_FIELD_PAD_H,
            sidebar_asks::BRIEF_FIELD_PAD_V,
            (brief.rect.w - 2.0 * sidebar_asks::BRIEF_FIELD_PAD_H).max(0.0),
            (brief.rect.h - 2.0 * sidebar_asks::BRIEF_FIELD_PAD_V).max(0.0),
        );
        brief.field.setFrame(inner.cg());
    }

    fn open_brief_field(&self, list: &SidebarListView, id: &str, text: &str, card: Rect, rect: Rect) {
        let mtm = MainThreadMarker::from(self);
        let box_ = NSView::new(mtm);
        box_.setWantsLayer(true);
        if let Some(layer) = box_.layer() {
            layer.setCornerRadius(6.0);
            layer.setBorderWidth(1.0);
            layer.setBackgroundColor(Some(&color(tokens::get().ground, 1.0).CGColor()));
            layer.setBorderColor(Some(&color(tokens::get().border_subtle, 1.0).CGColor()));
        }
        let font = self.ivars().shared.borrow().font(Style::Secondary).retain();
        let field = NSTextField::new(mtm);
        field.setBordered(false);
        field.setDrawsBackground(false);
        field.setFocusRingType(NSFocusRingType::None);
        field.setFont(Some(&font));
        field.setTextColor(Some(&color(tokens::get().text_primary, 1.0)));
        field.setUsesSingleLineMode(false);
        field.setMaximumNumberOfLines(0);
        if let Some(cell) = field.cell() {
            cell.setWraps(true);
            cell.setScrollable(false);
        }
        field.setStringValue(&NSString::from_str(text));
        field.setPlaceholderString(Some(&NSString::from_str("Your instruction")));
        unsafe { field.setDelegate(Some(ProtocolObject::from_ref(self))) };
        box_.addSubview(&field);
        box_.setFrame(rect.cg());
        list.addSubview(&box_);
        let open = BriefField { id: id.to_string(), box_, field, rect };
        Self::place_brief_text(&open);
        *self.ivars().brief_field.borrow_mut() = Some(open);
        log::info!("sidebar: brief field open {}", id);
        list.scrollRectToVisible(card.inset(-4.0).cg());
        let field = self.ivars().brief_field.borrow().as_ref().map(|open| open.field.retain());
        if let (Some(field), Some(win)) = (field, self.window()) {
            win.makeFirstResponder(Some(&field));
        }
    }

    fn close_brief_field(&self) {
        let Some(open) = self.ivars().brief_field.borrow_mut().take() else { return };
        log::info!("sidebar: brief field close {}", open.id);
        // The keyboard goes back to the terminal, then the box goes: removing
        // the first responder's view first would leave the window keyless.
        if let (Some(win), Some(kova)) = (self.window(), kova_of(self)) {
            win.makeFirstResponder(Some(&kova));
        }
        open.box_.removeFromSuperview();
    }

    /// The field's live text: kept in the window's state (so reopening Brief
    /// restores it) and in this view's model (so the card grows at once).
    fn brief_text_changed(&self) {
        let (id, text) = {
            let slot = self.ivars().brief_field.borrow();
            let Some(open) = slot.as_ref() else { return };
            let text = match open.field.currentEditor() {
                Some(editor) => editor.string().to_string(),
                None => open.field.stringValue().to_string(),
            };
            (open.id.clone(), text)
        };
        {
            let mut sh = self.ivars().shared.borrow_mut();
            if text.is_empty() {
                sh.model.asks.drafts.remove(&id);
            } else {
                sh.model.asks.drafts.insert(id.clone(), text.clone());
            }
        }
        if let Some(kova) = kova_of(self) {
            kova.sidebar_brief_draft(&id, &text);
        }
        self.relayout_list();
    }

    /// Enter, Cmd+Enter and Escape in the field. Enter is a new line in an
    /// instruction; the card's own field keeps Enter for sending. The field
    /// editor reports a Return under more than one selector, so all of them
    /// are the same key here.
    fn brief_command(&self, text_view: &NSTextView, selector: Sel) -> bool {
        if selector == sel!(cancelOperation:) {
            if let Some(key) = self.brief_key() {
                self.activate_brief(AsksHit::BriefCancel(key));
            }
            return true;
        }
        let newline = selector == sel!(insertNewline:)
            || selector == sel!(insertNewlineIgnoringFieldEditor:)
            || selector == sel!(insertLineBreak:);
        if !newline {
            return false;
        }
        let cmd = NSApplication::sharedApplication(MainThreadMarker::from(self))
            .currentEvent()
            .map(|event| event.modifierFlags().contains(NSEventModifierFlags::Command))
            .unwrap_or(false);
        if cmd {
            if let Some(key) = self.brief_key() {
                log::info!("sidebar: brief send (cmd+enter)");
                self.activate_brief(AsksHit::BriefSend(key));
            }
        } else {
            let _: () = unsafe { msg_send![text_view, insertNewlineIgnoringFieldEditor: None::<&AnyObject>] };
        }
        true
    }

    fn brief_key(&self) -> Option<u64> {
        let id = self.ivars().brief_field.borrow().as_ref().map(|open| open.id.clone())?;
        let sh = self.ivars().shared.borrow();
        sh.model.asks.cards.iter().find(|card| card.id == id).map(|card| card.key)
    }

    /// A command from the field goes through the list, which turns a hit
    /// into a click on the window's state.
    fn activate_brief(&self, hit: AsksHit) {
        if let Some(list) = self.ivars().list.get() {
            list.activate(ListHit::Ask(hit));
        }
    }

    /// Mirror the window's Asks state at once: waiting for the next tick
    /// would leave a click's open, pick or close invisible for a frame.
    pub(super) fn adopt_asks(&self, asks: sidebar_asks::AsksState) {
        {
            let mut sh = self.ivars().shared.borrow_mut();
            sh.model.asks = asks;
        }
        self.relayout_list();
    }

    fn draw_chrome(&self) {
        let sh = self.ivars().shared.borrow();
        let c = &sh.chrome;
        let b = self.bounds();
        // Ground and separator.
        fill_rect(&Rect::new(0.0, 0.0, b.size.width, b.size.height), tokens::get().ground, 1.0);
        fill_rect(&Rect::new(b.size.width - SEP_W, 0.0, SEP_W, b.size.height), tokens::get().separator, 1.0);

        // Top row: the view switcher at its right end, beside the traffic
        // lights. The selected icon sits on `SEL_BG`, the others are
        // tertiary and take a 10 % white ground under the mouse.
        for (rect, list) in c.switch.iter().zip(SWITCH_LISTS) {
            let selected = sh.model.view == list;
            let hovered = sh.chrome_hovered == ChromeHit::ViewSwitch(list);
            let pressed = sh.chrome_pressed == ChromeHit::ViewSwitch(list);
            if selected {
                fill_round(rect, SWITCH_RADIUS, tokens::get().sel_bg, 1.0);
            } else if hovered || pressed {
                fill_round(rect, SWITCH_RADIUS, tokens::get().white, if pressed { 0.16 } else { 0.10 });
            }
            let fg = if selected { tokens::get().sel_text } else { tokens::get().text_tertiary };
            let icon = match list {
                SidebarList::Panes => Icon::Sidebar,
                SidebarList::Asks => Icon::Message,
                SidebarList::Agents => Icon::Users,
                SidebarList::Scheduled => Icon::Clock,
            };
            stroke_icon(icon, rect, SWITCH_ICON_D, fg, if pressed { 0.85 } else { 1.0 });
            // The badge's ground ring first, then the badge: it has to read
            // over the selected ground as well as the sidebar's.
            let ring = |r: Rect, radius: f64| {
                fill_round(&r.inset(-SWITCH_BADGE_RING), radius + SWITCH_BADGE_RING, tokens::get().ground, 1.0);
            };
            match list {
                SidebarList::Asks if sh.model.asks_waiting > 0 => {
                    let label = sh.model.asks_waiting.to_string();
                    let w = (AppKitMetrics(&sh).width(&label, Style::PillBadge) + 8.0).max(SWITCH_BADGE_H);
                    let r = switch_badge(rect, w, SWITCH_BADGE_H);
                    ring(r, SWITCH_BADGE_H / 2.0);
                    fill_round(&r, SWITCH_BADGE_H / 2.0, tokens::get().accent, 1.0);
                    draw_text(&sh, &label, &r, Style::PillBadge, tokens::get().text_on_fill, 1.0, Align::Center, false);
                }
                SidebarList::Scheduled if sh.model.routine_failed => {
                    let r = switch_badge(rect, SWITCH_DOT_D, SWITCH_DOT_D);
                    ring(r, SWITCH_DOT_D / 2.0);
                    fill_round(&r, SWITCH_DOT_D / 2.0, tokens::get().error, 1.0);
                }
                _ => {}
            }
        }

        // Title row: the list's name, then the Next pill at the right.
        draw_text(&sh, sh.model.view.title(), &c.title, Style::ViewTitle, tokens::get().text_primary, 1.0, Align::Left, false);

        // Meta row: the summary on the left, the sort ghost on the right
        // (the Panes list only).
        let has_sort = c.sort.w > 0.0;
        let sort_hovered = has_sort && sh.chrome_hovered == ChromeHit::SortToggle;
        let sort_pressed = has_sort && sh.chrome_pressed == ChromeHit::SortToggle;
        if sort_hovered || sort_pressed {
            fill_round(&c.sort, SORT_RADIUS, tokens::get().white, if sort_pressed { 0.16 } else { 0.10 });
        }
        if has_sort {
            let sort_fg = if sort_hovered {
                tokens::get().text_primary
            } else if sh.model.sort == SidebarSort::Activity {
                tokens::get().accent
            } else {
                tokens::get().text_tertiary
            };
            let label = sh.model.sort.label();
            let text_w = AppKitMetrics(&sh).width(label, Style::Sort);
            let mut x = c.sort.x + (c.sort.w - (SORT_ICON_D + SORT_ICON_GAP + text_w)).max(0.0) / 2.0;
            stroke_icon(Icon::Sort, &Rect::new(x, c.sort.y, SORT_ICON_D, c.sort.h), SORT_ICON_D, sort_fg, 1.0);
            x += SORT_ICON_D + SORT_ICON_GAP;
            draw_text(&sh, label, &Rect::new(x, c.sort.y, text_w, c.sort.h), Style::Sort, sort_fg, 1.0, Align::Left, false);
        }
        let mut x = c.summary.x;
        let max_x = if has_sort { c.sort.x - 8.0 } else { c.summary.right() };
        if sh.model.view == SidebarList::Scheduled {
            // `N active · K failed` on the left, the clock it is phrased
            // against on the right.
            for (text, crit) in &sh.model.sched_meta {
                if x >= max_x {
                    break;
                }
                let fg = if *crit { tokens::get().crit_text } else { tokens::get().text_tertiary };
                let w = draw_text(&sh, text, &Rect::new(x, c.summary.y, max_x - x, c.summary.h), Style::Summary, fg, 1.0, Align::Left, false);
                x += w;
            }
            if let Some(clock) = &sh.model.meta_right {
                let w = AppKitMetrics(&sh).width(clock, Style::Summary);
                draw_text(&sh, clock, &Rect::new(c.summary.right() - w, c.summary.y, w, c.summary.h), Style::Summary, tokens::get().text_tertiary, 1.0, Align::Right, false);
            }
        } else {
            for (text, run) in &sh.model.summary {
                let fg = match run {
                    SummaryRun::Working => tokens::get().info_text,
                    SummaryRun::Waiting | SummaryRun::Idle | SummaryRun::Dot => tokens::get().text_tertiary,
                };
                if x >= max_x {
                    break;
                }
                let w = draw_text(&sh, text, &Rect::new(x, c.summary.y, max_x - x, c.summary.h), Style::Summary, fg, 1.0, Align::Left, false);
                x += w;
            }
        }

        // Next pill.
        {
            let hovered = sh.chrome_hovered == ChromeHit::NextPill;
            let pressed = sh.chrome_pressed == ChromeHit::NextPill;
            // Accent when there is something to read (darker under the
            // mouse), a plain sunken ground otherwise: not a button then, so
            // no hover and no press.
            let clickable = sh.model.pill.clickable();
            let (fill, text, badge): ([f32; 3], [f32; 3], Option<([f32; 3], [f32; 3])>) = match sh.model.pill {
                NextPill::Next(_) => (
                    if hovered { tokens::get().accent_pressed } else { tokens::get().accent },
                    tokens::get().text_on_fill,
                    Some((tokens::get().text_on_fill, tokens::get().accent)),
                ),
                NextPill::Nothing => (tokens::get().bg2, tokens::get().text_tertiary, None),
            };
            let text_alpha = if pressed && clickable { 0.85 } else { 1.0 };
            // Content-sized (`pill_width`), so the parts are laid end to
            // end from the left padding: play, label, badge.
            fill_round(&c.pill, PILL_H / 2.0, fill, 1.0);
            let mut right = c.pill.right() - PILL_PAD;
            if let (Some(n), Some((bg, fg))) = (sh.model.pill.badge(), badge) {
                let label = n.to_string();
                let w = badge_width(n, &AppKitMetrics(&sh));
                let r = Rect::new(right - w, c.pill.y + (PILL_H - PILL_BADGE_H) / 2.0, w, PILL_BADGE_H);
                fill_round(&r, PILL_BADGE_H / 2.0, bg, text_alpha);
                draw_text(&sh, &label, &r, Style::PillBadge, fg, 1.0, Align::Center, false);
                right -= w + PILL_BADGE_GAP;
            }
            let mut left = c.pill.x + PILL_PAD;
            if let Some(icon) = sh.model.pill.icon() {
                fill_icon(icon, &Rect::new(left, c.pill.y, PILL_ICON_D, c.pill.h), PILL_ICON_D, text, text_alpha);
                left += PILL_ICON_D + PILL_ICON_GAP;
            }
            let label = sh.model.pill.label();
            draw_text(&sh, label, &Rect::new(left, c.pill.y, (right - left).max(0.0), c.pill.h), Style::Pill, text, text_alpha, Align::Left, false);
        }
    }
}

// ---------------------------------------------------------------
// SidebarListView: groups and tiles
// ---------------------------------------------------------------

pub struct SidebarListViewIvars {
    shared: Rc<RefCell<Shared>>,
    /// The mouse went down inside the list: the matching mouse up is ours
    /// whatever it lands on.
    mouse_down: std::cell::Cell<bool>,
    /// While a tile is lifted: the local key monitor that turns Escape into
    /// a cancel (the list never takes first responder).
    escape_monitor: RefCell<Option<Retained<AnyObject>>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "KovaSidebarListView"]
    #[ivars = SidebarListViewIvars]
    pub struct SidebarListView;

    unsafe impl NSObjectProtocol for SidebarListView {}

    impl SidebarListView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            false
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: CGRect) {
            self.draw_list(dirty);
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            install_tracking_area(self);
        }

        /// The hand over every group block: the whole tab is a click target.
        /// In the scheduled view there are no blocks, and each row is the
        /// target; the two empty lists have neither.
        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            // Bind the rects first: adding a cursor rect is an AppKit call.
            let panels: Vec<CGRect> = {
                let sh = self.ivars().shared.borrow();
                if sh.layout.asks.shown {
                    sh.layout.asks.button_rects().iter().map(|r| r.cg()).collect()
                } else if matches!(sh.model.view, SidebarList::Scheduled | SidebarList::Agents) {
                    sh.layout.rows.iter().map(|r| r.frame.cg()).collect()
                } else {
                    // The needs card is clickable row by row, like a panel.
                    sh.layout.groups.iter().chain(sh.layout.needs_card.iter()).map(|r| r.cg()).collect()
                }
            };
            for r in panels {
                #[allow(deprecated)]
                self.addCursorRect_cursor(r, &NSCursor::pointingHandCursor());
            }
        }

        /// `NSToolTipOwner`: the tooltip of whatever sits under the pointer.
        /// A pane's name carries the folder and agent that used to be its
        /// subtitle; a button carries what it does.
        #[unsafe(method_id(view:stringForToolTip:point:userData:))]
        #[unsafe(method_family = none)]
        unsafe fn tooltip_string(&self, _view: &NSView, _tag: isize, point: CGPoint, _data: *mut std::ffi::c_void) -> Retained<NSString> {
            let sh = self.ivars().shared.borrow();
            let text = match sh.layout.hit(point.x, point.y) {
                ListHit::TileButton(_, b) => b.tooltip().to_string(),
                ListHit::Sched(SchedHit::Button(_, b)) => b.tip().to_string(),
                // The pane's own name line, where the folder rides.
                ListHit::Tile(id) if sh.layout.pane_name_rect(id).is_some_and(|r| r.contains(point.x, point.y)) => {
                    sh.model.groups.iter().flat_map(|g| g.tiles.iter()).find(|t| t.pane_id == id).map_or(String::new(), |t| t.tooltip.clone())
                }
                _ => String::new(),
            };
            NSString::from_str(&text)
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            let (x, y) = local_point(self, event);
            // Bind the hit first: the borrow must end before set_hovered takes it mutably.
            let hit = self.ivars().shared.borrow().layout.hit(x, y);
            self.set_hovered(hit);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.set_hovered(ListHit::Empty);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let (x, y) = local_point(self, event);
            let hit = self.ivars().shared.borrow().layout.hit(x, y);
            {
                let mut sh = self.ivars().shared.borrow_mut();
                sh.pressed = ListHit::Empty;
                sh.tab_drag = None;
                sh.pane_drag = None;
            }
            self.ivars().mouse_down.set(true);
            let double = event.clickCount() == 2;
            match hit {
                ListHit::Header(g) => {
                    if double {
                        // Bind before calling out: the borrow must be gone.
                        let tab_idx = self.ivars().shared.borrow().model.groups.get(g).map(|g| g.tab_idx);
                        if let (Some(tab_idx), Some(kova)) = (tab_idx, kova_of(self)) {
                            kova.do_switch_tab(tab_idx);
                            kova.start_rename_tab();
                        }
                    } else {
                        let mut sh = self.ivars().shared.borrow_mut();
                        sh.pressed = hit;
                        // Reordering only means something in tab order.
                        if sh.model.sort == SidebarSort::Kova {
                            if let Some(tab_id) = sh.model.groups.get(g).map(|g| g.tab_id) {
                                let row_y = sh.layout.row_for_group(g).map_or(y, |r| sh.layout.rows[r].frame.y);
                                sh.tab_drag = Some(TabDrag { tab_id, start_y: y, current_y: y, grab_offset: y - row_y, dragging: false });
                            }
                        }
                    }
                }
                ListHit::Tile(pane_id) => {
                    if double {
                        if let Some(kova) = kova_of(self) {
                            if kova.focus_pane_in_window(pane_id) {
                                kova.start_rename_pane();
                            }
                        }
                    } else {
                        let mut sh = self.ivars().shared.borrow_mut();
                        sh.pressed = hit;
                        if let Some(row) = sh.layout.row_for_pane(pane_id) {
                            let to = row - sh.layout.pane_run(row).start;
                            sh.pane_drag = Some(PaneDrag { pane_id, start_y: y, current_y: y, dragging: false, to });
                        }
                    }
                }
                hit if arms_press(hit) => {
                    self.ivars().shared.borrow_mut().pressed = hit;
                }
                _ => {}
            }
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            if !self.ivars().mouse_down.get() {
                return;
            }
            let (x, y) = local_point(self, event);
            let mut lifted = false;
            let mut just_lifted = false;
            {
                let mut sh = self.ivars().shared.borrow_mut();
                if let Some(mut d) = sh.tab_drag {
                    d.current_y = y;
                    if !d.dragging && (y - d.start_y).abs() >= DRAG_THRESHOLD {
                        d.dragging = true;
                    }
                    if d.dragging {
                        sh.pressed = ListHit::Empty;
                        lifted = true;
                    }
                    sh.tab_drag = Some(d);
                } else if let Some(mut d) = sh.pane_drag {
                    d.current_y = y;
                    if !d.dragging && (y - d.start_y).abs() >= DRAG_THRESHOLD {
                        d.dragging = true;
                        just_lifted = true;
                    }
                    if d.dragging {
                        sh.pressed = ListHit::Empty;
                        lifted = true;
                        // Aim: the rank the tile is over, with the run's
                        // hysteresis, against the current layout.
                        if let Some(frame) = sh.layout.pane_drag_frame(d.pane_id, y - d.start_y, d.to) {
                            d.to = frame.next_to(frame.dy);
                        }
                    }
                    sh.pane_drag = Some(d);
                } else if sh.pressed != ListHit::Empty {
                    let hit = sh.layout.hit(x, y);
                    if hit != sh.pressed {
                        sh.pressed = ListHit::Empty;
                    }
                }
            }
            if just_lifted {
                self.install_escape_monitor();
            }
            if lifted {
                // Near an edge of the visible part, keep scrolling.
                self.autoscroll(event);
            }
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            if !self.ivars().mouse_down.replace(false) {
                return;
            }
            let (x, y) = local_point(self, event);
            let (tab_drag, pane_drag, pressed) = {
                let mut sh = self.ivars().shared.borrow_mut();
                (sh.tab_drag.take(), sh.pane_drag.take(), std::mem::replace(&mut sh.pressed, ListHit::Empty))
            };
            self.remove_escape_monitor();
            if let Some(d) = tab_drag.filter(|d| d.dragging) {
                self.drop_tab(d.tab_id, y);
            } else if let Some(d) = pane_drag.filter(|d| d.dragging) {
                self.drop_pane(d.pane_id, d.to);
            } else if pressed != ListHit::Empty {
                let hit = self.ivars().shared.borrow().layout.hit(x, y);
                if hit == pressed {
                    self.activate(pressed);
                }
            }
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            let (x, y) = local_point(self, event);
            // Bind everything before the menus: they block, and the tick may
            // borrow the shared cell meanwhile.
            let (hit, tab_idx) = {
                let sh = self.ivars().shared.borrow();
                let hit = sh.layout.hit(x, y);
                let tab_idx = sh.layout.group_of(hit).and_then(|g| sh.model.groups.get(g)).map(|g| g.tab_idx);
                (hit, tab_idx)
            };
            let Some(kova) = kova_of(self) else { return };
            if let Some(pane_id) = hit.pane() {
                kova.show_sidebar_pane_menu(self, CGPoint { x, y }, pane_id);
            } else if let Some(tab_idx) = tab_idx {
                kova.show_sidebar_tab_menu(self, CGPoint { x, y }, tab_idx);
            }
        }
    }
);

impl SidebarListView {
    fn new(mtm: MainThreadMarker, shared: Rc<RefCell<Shared>>, frame: CGRect) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(SidebarListViewIvars {
            shared,
            mouse_down: std::cell::Cell::new(false),
            escape_monitor: RefCell::new(None),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn set_hovered(&self, hit: ListHit) {
        let mut sh = self.ivars().shared.borrow_mut();
        let tab = sh.layout.group_of(hit).and_then(|g| sh.model.groups.get(g)).map(|g| g.tab_id);
        if sh.hovered != hit || sh.hovered_tab != tab {
            sh.hovered = hit;
            sh.hovered_tab = tab;
            drop(sh);
            self.setNeedsDisplay(true);
        }
    }

    /// One tooltip zone per pane name and per action button, answered by
    /// `tooltip_string`. The buttons come last, so they win where a zone of
    /// theirs overlaps anything else.
    fn install_tooltips(&self) {
        // Collect first: the tooltip manager may call back into this view.
        let rects: Vec<CGRect> = {
            let sh = self.ivars().shared.borrow();
            let names = sh.layout.rows.iter().filter_map(|row| match row.kind {
                RowKind::Tile { pane_id, .. } => sh.layout.pane_name_rect(pane_id).map(|r| r.cg()),
                _ => None,
            });
            let buttons = sh.layout.rows.iter().flat_map(|row| {
                row.glyphs.iter().map(|(_, r)| r.cg()).chain(row.sched.iter().map(|(_, r, _)| r.cg()))
            });
            names.chain(buttons).collect()
        };
        self.removeAllToolTips();
        for r in rects {
            unsafe { self.addToolTipRect_owner_userData(r, self.as_ref(), std::ptr::null_mut()) };
        }
    }

    /// What a completed click does.
    fn activate(&self, hit: ListHit) {
        let Some(kova) = kova_of(self) else { return };
        let tab_of = |g: usize| self.ivars().shared.borrow().model.groups.get(g).map(|g| g.tab_idx);
        match hit {
            ListHit::Sched(SchedHit::Section(group)) => kova.sidebar_sched_section(group),
            ListHit::Sched(hit) => {
                // Bind the row before calling out: the borrow must be gone,
                // and the tick may re-lay the list out meanwhile.
                let row = {
                    let sh = self.ivars().shared.borrow();
                    let index = match hit {
                        SchedHit::Row(i) | SchedHit::Button(i, _) => i,
                        SchedHit::Section(_) => return,
                    };
                    sh.model.routines.get(index).map(|r| (r.name.clone(), r.log.clone()))
                };
                if let Some((name, log)) = row {
                    kova.sidebar_sched_hit(hit, &name, &log);
                    // The inline confirmation takes Escape, which the list
                    // never receives: it watches for the key itself.
                    if matches!(hit, SchedHit::Button(_, SchedButton::Delete)) {
                        self.install_escape_monitor();
                    }
                }
            }
            ListHit::Chevron(g) => {
                if let Some(t) = tab_of(g) {
                    kova.sidebar_toggle_collapsed(t);
                }
            }
            ListHit::HeaderDot(g) => {
                // Bind the anchor first: the menu blocks, and the tick may
                // re-lay the list out meanwhile.
                let anchor = {
                    let sh = self.ivars().shared.borrow();
                    sh.layout.row_for_group(g).map(|r| ListLayout::dot_button(&sh.layout.rows[r].frame))
                };
                if let (Some(t), Some(a)) = (tab_of(g), anchor) {
                    kova.show_sidebar_color_menu(self, CGPoint { x: a.x, y: a.bottom() + 2.0 }, t);
                }
            }
            ListHit::Header(g) => {
                if let Some(t) = tab_of(g) {
                    kova.sidebar_header_clicked(t);
                }
            }
            ListHit::Panel(g) => {
                if let Some(t) = tab_of(g) {
                    kova.do_switch_tab(t);
                }
            }
            ListHit::HeaderAdd(g) => {
                if let Some(t) = tab_of(g) {
                    kova.run_tab_action(t, TabAction::AddPane);
                }
            }
            ListHit::Tile(pane_id) => {
                kova.focus_pane_in_window(pane_id);
            }
            ListHit::TileButton(pane_id, button) => {
                let action = match button {
                    TileButton::Close => PaneAction::Close,
                    TileButton::Minimize => PaneAction::Minimize,
                    TileButton::Restore => PaneAction::Restore,
                    TileButton::Stop => PaneAction::Stop,
                    TileButton::StartClaude => PaneAction::StartClaude,
                    TileButton::Resume => PaneAction::Resume,
                    TileButton::MarkUnread | TileButton::MarkRead => PaneAction::ToggleUnread,
                };
                kova.dispatch_pane_action(pane_id, action);
            }
            ListHit::Need(index) => {
                let key = self.ivars().shared.borrow().model.needs.get(index).map(|r| r.key.clone());
                if let Some(key) = key {
                    kova.sidebar_open_need(&key);
                }
            }
            ListHit::NeedAll => kova.sidebar_toggle_needs_all(),
            ListHit::Agent(hit) => match hit {
                AgentsHit::Filter(section) => kova.sidebar_agents_filter(section),
                AgentsHit::Pane(pane_id) => {
                    kova.focus_pane_in_window(pane_id);
                }
                AgentsHit::Need(index) => {
                    let key = self.ivars().shared.borrow().model.agents.panes.get(index).and_then(|p| p.key.clone());
                    if let Some(key) = key {
                        kova.sidebar_open_need(&key);
                    }
                }
                AgentsHit::Tab(tab_idx) => kova.sidebar_agents_tab(tab_idx),
                AgentsHit::Open(reference) => {
                    let name = {
                        let sh = self.ivars().shared.borrow();
                        match reference {
                            sidebar_agents::RosterRef::Manager(i) => sh.model.agents.team.get(i).map(|r| r.name.clone()),
                            sidebar_agents::RosterRef::Dedicated(i) => {
                                sh.model.agents.dedicated.get(i).map(|r| r.name.clone())
                            }
                        }
                    };
                    if let Some(name) = name {
                        kova.sidebar_agents_open(&name);
                    }
                }
                AgentsHit::ToggleClosed => kova.sidebar_agents_toggle_closed(),
            },
            ListHit::NeedAnswer(pane_id) => {
                // The pane's ask row: the same landing, wherever the click
                // came from.
                let key = {
                    let sh = self.ivars().shared.borrow();
                    sh.model
                        .needs
                        .iter()
                        .find(|r| {
                            r.pane_id == Some(pane_id) && matches!(r.kind, NeedKind::Ask | NeedKind::Permission)
                        })
                        .map(|r| r.key.clone())
                };
                if let Some(key) = key {
                    kova.sidebar_open_need(&key);
                }
            }
            ListHit::Ask(hit) => kova.sidebar_ask_hit(hit),
            ListHit::Empty => {}
        }
    }

    /// Drop a dragged header: the tab moves to the insertion position under `y`.
    fn drop_tab(&self, tab_id: TabId, y: f64) {
        let (tab_idx, k) = {
            let sh = self.ivars().shared.borrow();
            let Some(g) = sh.model.groups.iter().find(|g| g.tab_id == tab_id) else { return };
            (g.tab_idx, sh.layout.insertion_index(y))
        };
        if let Some(kova) = kova_of(self) {
            kova.sidebar_reorder_tab(tab_idx, k);
        }
    }

    /// Drop a dragged tile at rank `to` of its column run. The pane may have
    /// gone since the mouse went down: then nothing happens.
    fn drop_pane(&self, pane_id: PaneId, to: usize) {
        let (tab_idx, ids, from, to) = {
            let sh = self.ivars().shared.borrow();
            let Some(frame) = sh.layout.pane_drag_frame(pane_id, 0.0, to) else { return };
            let Some(row) = sh.layout.row_for_pane(pane_id) else { return };
            let RowKind::Tile { group, .. } = sh.layout.rows[row].kind else { return };
            let Some(tab_idx) = sh.model.groups.get(group).map(|g| g.tab_idx) else { return };
            let ids: Vec<PaneId> = sh.layout.rows[frame.run.clone()]
                .iter()
                .filter_map(|r| match r.kind {
                    RowKind::Tile { pane_id, .. } => Some(pane_id),
                    _ => None,
                })
                .collect();
            (tab_idx, ids, frame.from, frame.to)
        };
        if from == to {
            return;
        }
        if let Some(kova) = kova_of(self) {
            kova.sidebar_drop_pane(tab_idx, &ids, from, to);
        }
    }

    /// Escape while a tile is lifted: the rows fall back into their order
    /// and nothing is committed.
    fn cancel_pane_drag(&self) {
        let cancelled = {
            let mut sh = self.ivars().shared.borrow_mut();
            sh.pane_drag.take().is_some_and(|d| d.dragging)
        };
        self.remove_escape_monitor();
        if cancelled {
            self.setNeedsDisplay(true);
        }
    }

    /// Escape while a delete is being confirmed: nothing is deleted.
    fn cancel_sched_confirm(&self) {
        let open = self.ivars().shared.borrow().model.sched.confirm.is_some();
        self.remove_escape_monitor();
        if open
            && let Some(kova) = kova_of(self)
        {
            kova.sidebar_sched_cancel_confirm();
        }
    }

    /// Watch for Escape while a tile is lifted. The list never becomes
    /// first responder, so the key would otherwise reach the terminal.
    fn install_escape_monitor(&self) {
        self.remove_escape_monitor();
        let view = self.retain();
        let handler = block2::RcBlock::new(move |event: std::ptr::NonNull<NSEvent>| -> *mut NSEvent {
            // Escape is key code 53.
            let key = unsafe { event.as_ref() }.keyCode();
            if key == 53 {
                view.cancel_pane_drag();
                view.cancel_sched_confirm();
                std::ptr::null_mut()
            } else {
                event.as_ptr()
            }
        });
        let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &handler) };
        *self.ivars().escape_monitor.borrow_mut() = monitor;
    }

    fn remove_escape_monitor(&self) {
        if let Some(monitor) = self.ivars().escape_monitor.borrow_mut().take() {
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
    }

    fn draw_list(&self, dirty: CGRect) {
        let sh = self.ivars().shared.borrow();
        if sh.layout.asks.shown {
            let visible = self.visibleRect();
            let scrim = Rect::new(visible.origin.x, visible.origin.y, visible.size.width, visible.size.height);
            sidebar_asks::draw(&sh, &sh.model.asks, &sh.layout.asks, sh.hovered, sh.pressed, scrim);
            return;
        }
        let dirty_top = dirty.origin.y;
        let dirty_bottom = dirty.origin.y + dirty.size.height;
        let visible = |r: &Rect| r.bottom() >= dirty_top && r.y <= dirty_bottom;

        // The `Needs you` card: the first thing in the Panes list.
        if let (Some(card), Some(header)) = (sh.layout.needs_card, sh.layout.needs_header)
            && visible(&card)
        {
            fill_round(&card, NEED_RADIUS, tokens::get().tile, 1.0);
            stroke_round(&card, NEED_RADIUS, 1.0, tokens::get().border_subtle, 1.0);
            self.draw_needs_header(&sh, &header);
        }

        // The active group is a raised card; the others have no fill.
        for (gi, block) in sh.layout.groups.iter().enumerate() {
            if !visible(block) {
                continue;
            }
            if sh.model.groups.get(gi).is_some_and(|g| g.active) {
                fill_round(block, PANEL_RADIUS, tokens::get().tile, 1.0);
                stroke_round(block, PANEL_RADIUS, 1.0, tokens::get().border_subtle, 1.0);
            }
        }

        // The lifted header or tile, resolved against the current layout: a
        // tick can re-lay the list out while the mouse is down.
        let lifted_tab = sh.tab_drag.filter(|d| d.dragging);
        let lifted_group = lifted_tab.and_then(|d| sh.model.groups.iter().position(|g| g.tab_id == d.tab_id));
        let lifted_pane = sh.pane_drag.filter(|d| d.dragging);
        let pane_frame = lifted_pane.and_then(|d| sh.layout.pane_drag_frame(d.pane_id, d.current_y - d.start_y, d.to));
        let lifted_row = pane_frame.as_ref().map(|f| f.run.start + f.from);
        for (ri, row) in sh.layout.rows.iter().enumerate() {
            let lifted = match row.kind {
                RowKind::Header { group } => lifted_group == Some(group),
                RowKind::Tile { .. } => lifted_row == Some(ri),
                RowKind::SchedHeader { .. }
                | RowKind::Routine { .. }
                | RowKind::Need { .. }
                | RowKind::NeedAll
                | RowKind::AgentStat(_)
                | RowKind::AgentHeader { .. }
                | RowKind::AgentPane { .. }
                | RowKind::AgentTab { .. }
                | RowKind::AgentTeam { .. }
                | RowKind::AgentDedicated { .. }
                | RowKind::AgentEmpty => false,
            };
            // The held tile is drawn later, floating; its skeleton takes
            // its place. The rows of its run step aside.
            if lifted && pane_frame.is_some() {
                continue;
            }
            let shift = pane_frame.as_ref().filter(|f| f.run.contains(&ri)).map_or(0.0, |f| f.moves[ri - f.run.start]);
            let y = row.frame.y + shift;
            if !visible(&Rect::new(row.frame.x, y, row.frame.w, row.frame.h)) {
                continue;
            }
            let alpha = if lifted { 0.35 } else { 1.0 };
            self.draw_row(&sh, row, y, alpha);
        }

        if let Some(hy) = sh.layout.hint_y {
            let r = Rect::new(PAD_H, hy, sh.layout.width - 2.0 * PAD_H, HINT_H);
            draw_text(&sh, hint_text(&sh.model), &r, Style::Hint, tokens::get().text_tertiary, 1.0, Align::Center, false);
        }

        // Drag feedback: the insertion line, then the floating copy.
        if let (Some(d), Some(g)) = (lifted_tab, lifted_group) {
            let k = sh.layout.insertion_index(d.current_y);
            let ly = sh.layout.insertion_line_y(k);
            fill_round(&Rect::new(PAD_H, ly - 1.0, sh.layout.width - 2.0 * PAD_H - (PAD_H - EDGE_TOLERANCE), 2.0), 1.0, tokens::get().accent, 1.0);
            if let Some(r) = sh.layout.row_for_group(g) {
                let row = &sh.layout.rows[r];
                self.draw_row(&sh, row, d.current_y - d.grab_offset, 0.9);
            }
        }
        if let (Some(f), Some(ri)) = (pane_frame, lifted_row) {
            let row = &sh.layout.rows[ri];
            let radius = TILE_RADIUS;
            // The skeleton: a dashed, empty outline the size of the tile
            // where it will land.
            let skeleton = Rect::new(row.frame.x, f.placeholder_y, row.frame.w, row.frame.h);
            stroke_dashed_round(&skeleton, radius, 1.0, tokens::get().white, 0.25);
            // The ghost: the tile lifted on a raised ground with a shadow,
            // a touch larger, following the cursor within its run.
            let y = row.frame.y + f.dy;
            let ghost = Rect::new(row.frame.x, y, row.frame.w, row.frame.h);
            NSGraphicsContext::saveGraphicsState_class();
            let lift = NSAffineTransform::transform();
            let (cx, cy) = (ghost.x + ghost.w / 2.0, ghost.y + ghost.h / 2.0);
            lift.translateXBy_yBy(cx, cy);
            lift.scaleBy(GHOST_SCALE);
            lift.translateXBy_yBy(-cx, -cy);
            lift.concat();
            fill_round_shadowed(&ghost, radius, tokens::get().tile, 1.0, 12.0, 0.35);
            self.draw_row(&sh, row, y, GHOST_ALPHA);
            NSGraphicsContext::restoreGraphicsState_class();
        }
    }

    /// Draw one row with its top at `y` (its own y, or the cursor's while
    /// dragged).
    fn draw_row(&self, sh: &Shared, row: &Row, y: f64, alpha: f64) {
        let dy = y - row.frame.y;
        let frame = Rect::new(row.frame.x, y, row.frame.w, row.frame.h);
        match row.kind {
            RowKind::SchedHeader { .. } | RowKind::Routine { .. } => {
                sidebar_sched::draw(sh, &sh.model.routines, &sh.model.sched, row, &frame, sh.hovered, sh.pressed, alpha);
            }
            RowKind::Header { group } => {
                if let Some(g) = sh.model.groups.get(group) {
                    self.draw_header(sh, g, group, &frame, alpha);
                }
            }
            RowKind::Tile { group, index, .. } => {
                if let Some(tile) = sh.model.groups.get(group).and_then(|g| g.tiles.get(index)) {
                    self.draw_tile(sh, tile, row, &frame, dy, alpha);
                }
            }
            RowKind::Need { index } => {
                if let Some(need) = sh.model.needs.get(index) {
                    self.draw_need(sh, need, row, &frame, dy, alpha);
                }
            }
            RowKind::NeedAll => {
                self.draw_need_all(sh, &frame, alpha);
            }
            RowKind::AgentStat(_)
            | RowKind::AgentHeader { .. }
            | RowKind::AgentPane { .. }
            | RowKind::AgentTab { .. }
            | RowKind::AgentTeam { .. }
            | RowKind::AgentDedicated { .. }
            | RowKind::AgentEmpty => {
                sidebar_agents::draw(
                    sh,
                    &sh.model.agents,
                    &sh.model.agent_state,
                    row,
                    &frame,
                    &sh.layout.agents.open_buttons,
                    &sh.layout.agents.open_waiting,
                    sh.hovered,
                    sh.pressed,
                    alpha,
                );
            }
        }
    }

    fn draw_header(&self, sh: &Shared, g: &GroupVm, group: usize, frame: &Rect, alpha: f64) {
        let tint = tab_tint(g.color);
        let selected = g.active;
        // The whole row, `+` included, counts as hovered: the `+` only shows
        // while the mouse is somewhere on the header.
        let row_hovered = sh.hovered.group() == Some(group);
        let body_pressed = matches!(sh.pressed, ListHit::Header(h) | ListHit::Chevron(h) if h == group);
        let add_hovered = sh.hovered == ListHit::HeaderAdd(group);
        let add_pressed = sh.pressed == ListHit::HeaderAdd(group);
        let dot_hovered = sh.hovered == ListHit::HeaderDot(group);
        let dot_pressed = sh.pressed == ListHit::HeaderDot(group);
        // Ground: a header has none of its own (the card shows through) and
        // lights up white 5 % under the mouse, as a rounded row.
        if body_pressed {
            fill_round(frame, HEADER_RADIUS, tokens::get().white, 0.10 * alpha);
        } else if row_hovered {
            fill_round(frame, HEADER_RADIUS, tokens::get().white, 0.05 * alpha);
        }
        let (muted, muted_alpha) = if selected { (tokens::get().white, 0.72) } else { (tokens::get().text_secondary, 1.0) };
        let (number_fg, number_alpha) = if selected { (tokens::get().white, 0.72) } else { (tokens::get().text_muted, 1.0) };
        let cy = frame.y + frame.h / 2.0;
        let chevron = if g.collapsed { Icon::ChevronRight } else { Icon::ChevronDown };
        stroke_icon(chevron, &ListLayout::header_chevron(frame), CHEVRON_D, muted, muted_alpha * alpha);
        // The tab's colour: a filled dot in the colour menu's box, which
        // stays the colour picker's target.
        let (dot_cx, _) = ListLayout::dot_centre(frame);
        fill_dot(dot_cx, cy, DOT_D, tint, alpha);
        if dot_hovered || dot_pressed {
            // The colour is a button: a thin white ring says so under the mouse.
            stroke_ring(dot_cx, cy, DOT_RING_D, 1.0, tokens::get().white, if dot_pressed { 0.35 } else { 0.20 } * alpha);
        }
        let x = frame.x + CHEVRON_ZONE_W + 16.0;

        // Right side: the pane count, or the `+` that takes its place while
        // the row is hovered. Its column ends where a tile's content does.
        let add = ListLayout::add_button(frame);
        let show_add = row_hovered || add_pressed;
        if show_add {
            if add_pressed {
                fill_round(&add, ADD_RADIUS, tokens::get().white, 0.20 * alpha);
            } else if add_hovered {
                fill_round(&add, ADD_RADIUS, tokens::get().white, 0.14 * alpha);
            }
            let plus_fg = if selected { tokens::get().white } else { tokens::get().text_primary };
            stroke_icon(Icon::Plus, &add, ICON_D, plus_fg, alpha);
        }
        let count_right = add.right();
        // The title never runs under the `+` icon while that one is up.
        let mut right = if show_add { add.x - 4.0 } else { count_right };
        if !show_add {
            // The `+` takes the count's place: the two never share the column.
            let count = g.summary.count.to_string();
            let w = draw_text(sh, &count, &Rect::new(x, frame.y, (count_right - x).max(0.0), frame.h), Style::Number, number_fg, number_alpha * alpha, Align::Right, false);
            right = count_right - w - 6.0;
            if g.collapsed {
                // Folded: a dot per pane that waits or works, left of the count.
                if g.summary.working > 0 {
                    fill_dot(right - 3.0, cy, 6.0, tokens::get().working, alpha);
                    right -= 10.0;
                }
                if g.summary.awaiting > 0 {
                    fill_dot(right - 3.0, cy, 6.0, tokens::get().awaiting, alpha);
                    right -= 10.0;
                }
            }
        }
        let title_fg = if selected { tokens::get().white } else { tokens::get().text_primary };
        draw_text(sh, &g.title, &Rect::new(x, frame.y, (right - x).max(0.0), frame.h), Style::Title, title_fg, alpha, Align::Left, g.renaming);
    }

    /// The block's header line: `Needs you` on the left, the counts right
    /// aligned against the card's edge and cut there when they do not fit.
    fn draw_needs_header(&self, sh: &Shared, header: &Rect) {
        let m = AppKitMetrics(sh);
        let label_w =
            draw_text(sh, NEED_TITLE, &Rect::new(header.x + NEED_ROW_PAD_H, header.y, header.w, header.h), Style::Sort, tokens::get().text_secondary, 1.0, Align::Left, false);
        let runs = needs_summary(&sh.model.needs);
        let total: f64 = runs.iter().map(|(text, _)| m.width(text, Style::Sort)).sum();
        let min_x = header.x + NEED_ROW_PAD_H + label_w + NEED_GAP;
        let max_x = header.right() - NEED_ROW_PAD_H;
        let mut x = (max_x - total).max(min_x);
        for (text, run) in runs {
            if x >= max_x {
                break;
            }
            let fg = match run {
                NeedTally::You => tokens::get().warn_text,
                NeedTally::Problem => tokens::get().crit_text,
                NeedTally::Dot => tokens::get().text_tertiary,
            };
            draw_text(sh, &text, &Rect::new(x, header.y, (max_x - x).max(0.0), header.h), Style::Sort, fg, 1.0, Align::Left, false);
            x += m.width(&text, Style::Sort);
        }
    }

    /// One row of the block: the state dot (or the failed run's x-circle),
    /// the title, the detail and the age.
    fn draw_need(&self, sh: &Shared, need: &NeedRowVm, row: &Row, frame: &Rect, dy: f64, alpha: f64) {
        let RowKind::Need { index } = row.kind else { return };
        let hovered = sh.hovered == ListHit::Need(index);
        let pressed = sh.pressed == ListHit::Need(index);
        if pressed {
            fill_round(frame, NEED_ROW_RADIUS, tokens::get().tile_pressed, alpha);
        } else if hovered {
            fill_round(frame, NEED_ROW_RADIUS, tokens::get().tile_hover, alpha);
        }
        let cy = frame.y + frame.h / 2.0;
        let dot_cx = frame.x + NEED_ROW_PAD_H + GLYPH_BOX_W / 2.0;
        let waiting = matches!(need.kind, NeedKind::Ask | NeedKind::Permission | NeedKind::Attention);
        if need.kind == NeedKind::FailedRoutine {
            // A scheduled run has no pane: the x-circle says the run died.
            let box_ = Rect::new(dot_cx - ICON_D / 2.0, cy - ICON_D / 2.0, ICON_D, ICON_D);
            stroke_icon(Icon::XCircle, &box_, ICON_D, tokens::get().error, alpha);
        } else {
            fill_dot(dot_cx, cy, GLYPH_D, if waiting { tokens::get().awaiting } else { tokens::get().error }, alpha);
        }
        let m = AppKitMetrics(sh);
        let text_x = frame.x + NEED_ROW_PAD_H + GLYPH_BOX_W;
        let right = frame.right() - NEED_ROW_PAD_H;
        let age_w = m.width(&need.age, Style::Number);
        let age_x = right - age_w;
        let title_right = if row.detail.w > 0.0 { row.detail.x - NEED_GAP } else { age_x - NEED_GAP };
        draw_text(sh, &need.title, &Rect::new(text_x, frame.y, (title_right - text_x).max(0.0), frame.h), Style::Title, tokens::get().text_primary, alpha, Align::Left, false);
        if row.detail.w > 0.0 {
            let detail = Rect::new(row.detail.x, row.detail.y + dy, row.detail.w, row.detail.h);
            draw_text(sh, &need.detail, &detail, Style::Secondary, tokens::get().text_tertiary, alpha, Align::Left, false);
        }
        let age_fg = if waiting { tokens::get().warn_text } else { tokens::get().crit_text };
        draw_text(sh, &need.age, &Rect::new(age_x, frame.y, age_w, frame.h), Style::Number, age_fg, alpha, Align::Right, false);
    }

    /// The block's ghost button: `Show all N` while folded, `Show less` once
    /// every row is shown.
    fn draw_need_all(&self, sh: &Shared, frame: &Rect, alpha: f64) {
        let hovered = sh.hovered == ListHit::NeedAll;
        let pressed = sh.pressed == ListHit::NeedAll;
        if pressed {
            fill_round(frame, NEED_ROW_RADIUS, tokens::get().tile_pressed, alpha);
        } else if hovered {
            fill_round(frame, NEED_ROW_RADIUS, tokens::get().tile_hover, alpha);
        }
        let label = need_all_label(sh.model.needs.len(), sh.model.needs_all);
        let fg = if hovered { tokens::get().text_primary } else { tokens::get().text_secondary };
        let inner = Rect::new(frame.x + NEED_ROW_PAD_H, frame.y, (frame.w - 2.0 * NEED_ROW_PAD_H).max(0.0), frame.h);
        draw_text(sh, &label, &inner, Style::Link, fg, alpha, Align::Left, false);
    }

    fn draw_tile(&self, sh: &Shared, tile: &TileVm, row: &Row, frame: &Rect, dy: f64, alpha: f64) {
        let hovered = sh.hovered.pane() == Some(tile.pane_id);
        let pressed = sh.pressed == ListHit::Tile(tile.pane_id);
        let shifted = |r: &Rect| Rect::new(r.x, r.y + dy, r.w, r.h);

        // A tile has no ground of its own: the group's card shows through.
        // The mouse lights a hover ground; the focused pane keeps a neutral
        // one, one step off the hover, and the rail at its left edge is what
        // marks the selection (no filled blue row).
        if tile.focused {
            fill_round(frame, TILE_RADIUS, tokens::get().sel_row_bg, alpha);
            let rail = Rect::new(
                frame.x + RAIL_INSET,
                frame.y + (frame.h - RAIL_H) / 2.0,
                RAIL_W,
                RAIL_H,
            );
            fill_round(&rail, RAIL_W / 2.0, tokens::get().sel_row_rail, alpha);
        } else if pressed {
            fill_round(frame, TILE_RADIUS, tokens::get().tile_pressed, alpha);
        } else if hovered {
            fill_round(frame, TILE_RADIUS, tokens::get().tile_hover, alpha);
        }
        let (title_fg, sec_fg, meta_fg) = if tile.focused {
            (tokens::get().sel_row_text, tokens::get().sel_row_text, tokens::get().sel_row_text)
        } else {
            (tokens::get().text_primary, tokens::get().text_secondary, tokens::get().text_muted)
        };

        // One line: the state dot in its box, the session name, and the right
        // column (the wait, the call to action, the state word).
        let text_x = frame.x + TEXT_X;
        let right = frame.right() - TILE_PAD_H;
        let line = Rect::new(text_x, frame.y + TILE_PAD_V, (right - text_x).max(0.0), LINE_TITLE_H);
        let dot_cx = frame.x + TILE_PAD_H + GLYPH_BOX_W / 2.0;
        let cy = line.y + LINE_TITLE_H / 2.0;
        // Only a pane that needs the user keeps its dot (a question, an error):
        // idle, working and unread said nothing the row does not already say
        // (the state word, the unread dot on the right).
        if matches!(tile.state, TileState::Awaiting | TileState::Error) {
            fill_dot(dot_cx, cy, GLYPH_D, tile.state.color(), alpha);
        }

        // The right column, laid out right to left, then the name in what is
        // left of it. The unread dot comes before the column.
        let col = tile_column(tile, right, line.y, LINE_TITLE_H, &AppKitMetrics(sh));
        let col_x = if hovered { col.hover_x() } else { col.x };
        let mut name_right = col_x;
        if let Some((label, r)) = col.wait.as_ref().filter(|_| !hovered) {
            draw_text(sh, label, &shifted(r), Style::Secondary, tokens::get().warn_text, alpha, Align::Left, false);
        }
        if let Some(area) = col.answer {
            let area = shifted(&area);
            if !hovered {
                draw_text(sh, "\u{b7}", &Rect::new(area.x - 9.0, area.y, 8.0, LINE_TITLE_H), Style::Secondary, sec_fg, alpha, Align::Left, false);
            }
            let link_hovered = sh.hovered == ListHit::NeedAnswer(tile.pane_id);
            draw_text(sh, NEED_ANSWER, &area, Style::Link, link_ink(tile.focused, link_hovered), alpha, Align::Left, false);
        }
        if let Some((button, label, r)) = &col.link {
            let r = shifted(r);
            let link_hovered = sh.hovered == ListHit::TileButton(tile.pane_id, *button);
            draw_link(sh, button.icon(), label, &r, link_ink(tile.focused, link_hovered), alpha);
        }
        if let Some((text, r)) = &col.meta {
            draw_text(sh, text, &shifted(r), Style::Number, meta_fg, alpha, Align::Right, false);
        }
        if matches!(tile.state, TileState::Unread(_)) {
            let x = col_x - 8.0 - UNREAD_DOT_D / 2.0;
            fill_dot(x, cy, UNREAD_DOT_D, tokens::get().accent, alpha);
            name_right = x - UNREAD_DOT_D / 2.0 - 4.0;
        }

        // The hover buttons sit left of the column, never over it.
        if hovered {
            for (b, r) in &row.glyphs {
                let r = shifted(r);
                let b_hovered = sh.hovered == ListHit::TileButton(tile.pane_id, *b);
                let b_pressed = sh.pressed == ListHit::TileButton(tile.pane_id, *b);
                // Bare icons; a white ground only under the one the mouse is on.
                if b_pressed {
                    fill_round(&r, BUTTON_RADIUS, tokens::get().white, 0.20 * alpha);
                } else if b_hovered {
                    fill_round(&r, BUTTON_RADIUS, tokens::get().white, 0.12 * alpha);
                }
                let (fg, fg_alpha) = match b {
                    TileButton::Close if b_hovered => (tokens::get().error, 1.0),
                    TileButton::Stop => (tokens::get().interrupt, 1.0),
                    _ if b_hovered => (tokens::get().text_primary, 1.0),
                    _ => (sec_fg, 1.0),
                };
                stroke_icon(b.icon(), &r, ICON_D, fg, fg_alpha * alpha);
                name_right = name_right.min(r.x - 4.0);
            }
        }

        // The name: the session name, one line, `⊟ ` when minimized and `★`
        // when bookmarked.
        let mut sx = text_x;
        if tile.bookmarked {
            let w = draw_text(sh, "\u{2605}", &Rect::new(sx, line.y, 14.0, LINE_TITLE_H), Style::Secondary, tokens::get().awaiting, alpha, Align::Left, false);
            sx += w + 4.0;
        }
        let title = if tile.minimized { format!("\u{229f} {}", tile.title) } else { tile.title.clone() };
        draw_text(sh, &title, &Rect::new(sx, line.y, (name_right - sx).max(0.0), LINE_TITLE_H), Style::Title, title_fg, alpha, Align::Left, tile.renaming);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::sidebar::{CollapsedSummary, PaneFlags, UnreadKind};

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

    #[test]
    fn hovering_a_waiting_tile_never_hides_its_name() {
        let mut t = tile(7, 0, TileState::Awaiting);
        t.waiting = Some("13h".into());
        for w in [200.0, 240.0, 280.0, 360.0] {
            let row = tile_row(0, 0, &t, 0.0, 0.0, w, &FakeMetrics);
            let text_x = TEXT_X;
            // The buttons sit right of a name kept at TITLE_MIN_W, and left of `Answer`.
            if let Some((_, leftmost)) = row.glyphs.last() {
                assert!(leftmost.x - text_x >= TITLE_MIN_W - 0.01, "width {w}: name squeezed");
            }
            let answer = row.answer.unwrap();
            for (_, r) in &row.glyphs {
                assert!(r.right() <= answer.x - COLUMN_GAP + 0.01, "width {w}: a button over Answer");
            }
        }
        // Wide enough, all four buttons (the age gives way while hovered).
        assert_eq!(tile_row(0, 0, &t, 0.0, 0.0, 280.0, &FakeMetrics).glyphs.len(), 4);
        // Narrow, the leftmost go first, Close always last to go.
        let narrow = tile_row(0, 0, &t, 0.0, 0.0, 200.0, &FakeMetrics);
        assert!(narrow.glyphs.len() < 4);
        assert_eq!(narrow.glyphs.first().map(|(b, _)| *b), Some(TileButton::Close));
        assert_eq!(hover_glyph_count(0.0, TITLE_MIN_W - 1.0, 4), 0);
    }

    fn tile(pane_id: PaneId, column: usize, state: TileState) -> TileVm {
        TileVm {
            pane_id,
            column,
            state,
            unread: matches!(state, TileState::Unread(_)),
            minimized: false,
            bare_shell: state == TileState::Shell,
            resumable: false,
            title: format!("pane {pane_id}"),
            tooltip: "link \u{b7} claude".into(),
            agent: Some("claude".into()),
            bookmarked: false,
            focused: false,
            renaming: false,
            question: None,
            detail: None,
            summary: None,
            waiting: None,
        }
    }

    fn group(tab_idx: usize, collapsed: bool, tiles: Vec<TileVm>) -> GroupVm {
        GroupVm {
            tab_idx,
            tab_id: 100 + tab_idx as u32,
            title: format!("tab {tab_idx}"),
            color: Some(tab_idx),
            active: tab_idx == 0,
            collapsed,
            renaming: false,
            summary: CollapsedSummary { awaiting: 0, working: 0, count: tiles.len() },
            tiles: if collapsed { Vec::new() } else { tiles },
        }
    }

    #[test]
    fn every_clickable_target_is_armed_on_press_or_arms_itself() {
        // A target that neither arms here nor starts a drag is dead on click
        // while hovering perfectly — the bug this guards is silent.
        for hit in [
            ListHit::Chevron(0),
            ListHit::HeaderDot(0),
            ListHit::HeaderAdd(0),
            ListHit::TileButton(1, TileButton::Stop),
            ListHit::Panel(0),
            ListHit::Sched(SchedHit::Row(0)),
            ListHit::Need(0),
            ListHit::NeedAll,
            ListHit::NeedAnswer(1),
            ListHit::Ask(AsksHit::ToggleSnoozed),
            ListHit::Agent(AgentsHit::Filter(sidebar_agents::AgentSection::Working)),
            ListHit::Agent(AgentsHit::Pane(1)),
            ListHit::Agent(AgentsHit::Need(0)),
            ListHit::Agent(AgentsHit::Tab(0)),
            ListHit::Agent(AgentsHit::Open(sidebar_agents::RosterRef::Manager(0))),
            ListHit::Agent(AgentsHit::ToggleClosed),
        ] {
            assert!(arms_press(hit), "{hit:?} would never reach activate()");
        }
        // These two arm themselves, because a press on them may turn into a
        // drag; empty space arms nothing.
        assert!(!arms_press(ListHit::Header(0)));
        assert!(!arms_press(ListHit::Tile(1)));
        assert!(!arms_press(ListHit::Empty));
    }

    #[test]
    fn the_scheduled_view_lays_out_through_its_sections() {
        let mut m = model();
        m.view = SidebarList::Scheduled;
        m.routines_loaded = true;
        m.routines = vec![
            RoutineVm { name: "alpha-digest".into(), group: crate::routines::Group::Today, ..RoutineVm::default() },
            RoutineVm { name: "beta-weekly".into(), group: crate::routines::Group::Today, ..RoutineVm::default() },
        ];
        let l = ListLayout::new(&m, 260.0, &FakeMetrics);
        // One header per non-empty section, then its rows.
        assert_eq!(l.sched.sections.len(), 1);
        assert_eq!(l.sched.sections[0].group, crate::routines::Group::Today);
        assert_eq!(l.sched.sections[0].count, 2);
        assert_eq!(l.rows[0].kind, RowKind::SchedHeader { group: crate::routines::Group::Today, count: 2 });
        assert_eq!(l.rows[1].kind, RowKind::Routine { index: 0 });
        assert_eq!(l.rows[2].kind, RowKind::Routine { index: 1 });
        assert!(l.groups.is_empty());
        assert_eq!(l.hint_y, None);
        assert_eq!(l.rows[1].frame.h, sidebar_sched::ROW_H);
        // A click on the header folds the section; a click on the row opens
        // it, and that is all it does.
        let h = &l.rows[0].frame;
        assert_eq!(l.hit(h.x + 5.0, h.y + 5.0), ListHit::Sched(SchedHit::Section(crate::routines::Group::Today)));
        let r = &l.rows[1].frame;
        assert_eq!(l.hit(r.x + 5.0, r.y + 5.0), ListHit::Sched(SchedHit::Row(0)));

        // The open row grows by its body and pushes the next one down.
        m.routines[0].expanded = true;
        let open = ListLayout::new(&m, 260.0, &FakeMetrics);
        assert!(open.rows[1].frame.h > sidebar_sched::ROW_H);
        assert_eq!(open.rows[2].frame.h, sidebar_sched::ROW_H);
        assert!(open.content_h > l.content_h);
        // Its whole height is the hit region, body included.
        let br = &open.rows[1].frame;
        assert_eq!(open.hit(br.x + 5.0, br.bottom() - 2.0), ListHit::Sched(SchedHit::Row(0)));

        // Folded, the section keeps its header and drops its rows.
        m.sched.toggle(crate::routines::Group::Today);
        let folded = ListLayout::new(&m, 260.0, &FakeMetrics);
        assert_eq!(folded.rows.len(), 1);
        assert!(folded.sched.sections[0].folded);

        // With no routines at all, the empty line takes the hint's slot.
        m.routines.clear();
        let empty = ListLayout::new(&m, 260.0, &FakeMetrics);
        assert!(empty.rows.is_empty());
        assert_eq!(empty.hint_y, Some(LIST_TOP));
    }

    /// Tab 0 expanded with two tiles, tab 1 collapsed, tab 2 with one tile.
    fn model() -> SidebarModel {
        SidebarModel {
            view: SidebarList::default(),
            routines: Vec::new(),
            routines_loaded: false,
            asks_waiting: 0,
            routine_failed: false,
            summary: Vec::new(),
            sort: SidebarSort::Kova,
            pill: NextPill::Nothing,
            groups: vec![
                group(0, false, vec![tile(10, 0, TileState::Idle), tile(11, 0, TileState::Working)]),
                group(1, true, vec![tile(20, 0, TileState::Idle)]),
                group(2, false, vec![tile(30, 0, TileState::Shell)]),
            ],
            needs: Vec::new(),
            needs_all: false,
            asks: Default::default(),
            show_hint: false,
            sched: Default::default(),
            sched_meta: Vec::new(),
            agents: Default::default(),
            agent_state: Default::default(),
            meta_right: None,
        }
    }

    const W: f64 = 280.0;
    /// Left edge of a group's card.
    const SX: f64 = 12.0;
    /// Right edge of a group's card: the list's edge, 8 in.
    const SR: f64 = W - 8.0;
    /// Left edge of a row: the header and the tiles share it, 8 under the
    /// card's edge.
    const TX: f64 = SX + ROW_INSET;
    /// Right edge of a row, inset like the left one.
    const TR: f64 = SR - ROW_INSET;

    #[test]
    fn rows_stack_in_group_blocks() {
        let l = ListLayout::new(&model(), W, &FakeMetrics);
        let ys: Vec<f64> = l.rows.iter().map(|r| r.frame.y).collect();
        // Group from 6: 4 pt padding, header 10..38, 4 pt, tile 42..68, 4 pt,
        // tile 72..98, 4 pt (the group ends at 102); 8 pt gap, group 110:
        // header 114..142 (folded, 4 pt under it, ends 146); 8 pt gap, group
        // 154: header 158..186, 4 pt, tile 190..216, 4 pt (ends 220); 12 pt
        // under the list.
        assert_eq!(ys, vec![10.0, 42.0, 72.0, 114.0, 158.0, 190.0]);
        assert_eq!(l.rows[1].frame.h, TILE_HEIGHT);
        // Every row of a group, header included, sits on the same inset:
        // their grounds line up, selected or not.
        for row in &l.rows {
            assert_eq!(row.frame.x, TX);
            assert_eq!(row.frame.right(), TR);
        }
        assert_eq!(l.groups[0], Rect::new(SX, 6.0, SR - SX, 96.0));
        assert_eq!(l.groups[1], Rect::new(SX, 110.0, SR - SX, 36.0));
        assert_eq!(l.groups[2], Rect::new(SX, 154.0, SR - SX, 66.0));
        assert_eq!(l.content_h, 220.0 + 12.0);
        assert_eq!(l.hint_y, None);
        // The selection changes the card, not the geometry.
        let mut m = model();
        m.groups[0].active = false;
        m.groups[2].active = true;
        assert_eq!(ListLayout::new(&m, W, &FakeMetrics).rows, l.rows);
    }

    #[test]
    fn every_tile_is_one_line_of_the_same_height() {
        let mut m = model();
        let mut awaiting = tile(1, 0, TileState::Awaiting);
        awaiting.question = Some("Should I overwrite hello.txt with the new content?".into());
        awaiting.detail = Some("Write hello.txt".into());
        awaiting.waiting = Some("4m".into());
        let mut unread = tile(2, 0, TileState::Unread(UnreadKind::Done));
        unread.summary = Some("Pushed the copy".into());
        m.groups = vec![group(0, false, vec![awaiting, unread, tile(4, 0, TileState::Working)])];
        let l = ListLayout::new(&m, W, &FakeMetrics);
        let hs: Vec<f64> = l.rows.iter().map(|r| r.frame.h).collect();
        // One 18 pt name line, 4 pt either side: 26 for every state, the
        // waiting one included. Nothing rides under the name any more.
        assert_eq!(hs, vec![HEADER_H, TILE_HEIGHT, TILE_HEIGHT, TILE_HEIGHT]);
        assert_eq!(l.rows[1].frame.h, 26.0);
        assert!(l.rows[1].actions.is_empty());
    }

    #[test]
    fn the_hint_sits_under_the_last_group() {
        let mut m = model();
        m.show_hint = true;
        let l = ListLayout::new(&m, W, &FakeMetrics);
        assert_eq!(l.hint_y, Some(220.0 + GROUP_GAP));
        assert_eq!(l.content_h, 220.0 + GROUP_GAP + 20.0 + 12.0);
        // An empty window still lays out.
        m.groups.clear();
        let l = ListLayout::new(&m, W, &FakeMetrics);
        assert_eq!(l.hint_y, Some(6.0));
        assert!(l.rows.is_empty());
    }

    #[test]
    fn hit_tells_the_rows_and_their_buttons_apart() {
        let l = ListLayout::new(&model(), W, &FakeMetrics);
        // Header (20, 10, 244, 28): the chevron, the colour dot, the body,
        // the `+`.
        assert_eq!(ListLayout::header_chevron(&l.rows[0].frame), Rect::new(TX + 2.0, 18.0, 12.0, 12.0));
        assert_eq!(l.hit(TX + 5.0, 22.0), ListHit::Chevron(0));
        assert_eq!(l.hit(TX + 40.0, 22.0), ListHit::Header(0));
        // The dot is 8 pt, centred in its 16 pt box: that box is the colour
        // menu's target, right after the chevron zone.
        assert_eq!(ListLayout::dot_centre(&l.rows[0].frame), (TX + 22.0, 24.0));
        assert_eq!(ListLayout::dot_button(&l.rows[0].frame), Rect::new(TX + 14.0, 16.0, 16.0, 16.0));
        assert_eq!(ListLayout::dot_rect(&l.rows[0].frame), Rect::new(TX + 18.0, 20.0, DOT_D, DOT_D));
        assert_eq!(l.hit(TX + 22.0, 24.0), ListHit::HeaderDot(0));
        assert_eq!(l.hit(TX + 16.0, 18.0), ListHit::HeaderDot(0));
        assert_eq!(l.hit(TX + 22.0, 12.0), ListHit::Header(0));
        assert_eq!(l.hit(TX + 32.0, 24.0), ListHit::Header(0));
        let add = ListLayout::add_button(&l.rows[0].frame);
        assert_eq!(add, Rect::new(TR - ADD_INSET - ADD_D, 10.0 + (HEADER_H - ADD_D) / 2.0, ADD_D, ADD_D));
        assert_eq!(l.hit(add.x + 3.0, add.y + 3.0), ListHit::HeaderAdd(0));
        // Hovering the `+` or the bar still counts as hovering the header
        // row (the `+` shows while the mouse is anywhere on the row), and
        // the row's hover tells the `+` and the bar from the body.
        assert_eq!(ListHit::HeaderAdd(0).group(), Some(0));
        assert_eq!(ListHit::HeaderDot(0).group(), Some(0));
        assert_eq!(ListHit::Header(0).group(), Some(0));
        assert_eq!(ListHit::Chevron(0).group(), Some(0));
        assert_eq!(ListHit::Tile(10).group(), None);
        assert_ne!(l.hit(add.x + 3.0, add.y + 3.0), l.hit(add.x - 3.0, add.y + 3.0));
        assert_eq!(l.hit(add.x - 3.0, add.y + 3.0), ListHit::Header(0));
        // The group's 4 pt padding, the 4 pt between two rows and the 8 pt
        // side inset are the panel; the body is the pane.
        assert_eq!(l.hit(100.0, 7.0), ListHit::Panel(0));
        assert_eq!(l.hit(100.0, 39.0), ListHit::Panel(0));
        assert_eq!(l.hit(100.0, 43.0), ListHit::Tile(10));
        assert_eq!(l.hit(100.0, 91.0), ListHit::Tile(11));
        assert_eq!(l.hit(15.0, 60.0), ListHit::Panel(0));
        assert_eq!(l.hit(100.0, 100.0), ListHit::Panel(0));
        assert_eq!(l.hit(100.0, 112.0), ListHit::Panel(1));
        assert_eq!(l.hit(100.0, 130.0), ListHit::Header(1));
        assert_eq!(l.hit(25.0, 200.0), ListHit::Tile(30));
        // Left of the groups, between two of them, past the last: nothing.
        assert_eq!(l.hit(5.0, 60.0), ListHit::Empty);
        assert_eq!(l.hit(100.0, 150.0), ListHit::Empty);
        assert_eq!(l.hit(100.0, 1000.0), ListHit::Empty);
        // Every hit on a panel names its group, a tile's through its row;
        // only the header parts count as the header row.
        assert_eq!(l.group_of(ListHit::Panel(1)), Some(1));
        assert_eq!(l.group_of(ListHit::Tile(30)), Some(2));
        assert_eq!(l.group_of(ListHit::TileButton(11, TileButton::Close)), Some(0));
        assert_eq!(l.group_of(ListHit::HeaderDot(1)), Some(1));
        assert_eq!(l.group_of(ListHit::Tile(99)), None);
        assert_eq!(l.group_of(ListHit::Empty), None);
        assert_eq!(ListHit::Panel(1).group(), None);
        // Working tile: close, minimize, mark unread, 22 pt boxes left of its
        // `working` column, centred on the row; Stop, the leftmost, gives way
        // at this width so the name keeps TITLE_MIN_W.
        let row = &l.rows[2];
        let boxes: Vec<TileButton> = row.glyphs.iter().map(|(b, _)| *b).collect();
        assert_eq!(boxes, vec![TileButton::Close, TileButton::Minimize, TileButton::MarkUnread]);
        let column_w = FakeMetrics.width("working", Style::Number);
        let (_, close) = row.glyphs[0];
        assert_eq!(close.right(), TR - TILE_PAD_H - COLUMN_GAP - column_w);
        assert_eq!(close.w, BUTTON_D);
        assert_eq!(close.y, row.frame.y + TILE_PAD_V + (LINE_TITLE_H - BUTTON_D) / 2.0);
        assert_eq!(l.hit(close.x + 5.0, close.y + 5.0), ListHit::TileButton(11, TileButton::Close));
        let (_, mark) = row.glyphs[2];
        // Minimize sits one gap left of close, mark one gap left of minimize.
        assert_eq!(mark.right(), close.x - 2.0 * BUTTON_GAP - BUTTON_D);
        assert_eq!(l.hit(mark.x + 5.0, mark.y + 5.0), ListHit::TileButton(11, TileButton::MarkUnread));
        // A wider list has room for Stop too.
        let wide = ListLayout::new(&model(), W + 40.0, &FakeMetrics);
        let (b, stop) = wide.rows[2].glyphs[3];
        assert_eq!(b, TileButton::Stop);
        assert_eq!(wide.hit(stop.x + 5.0, stop.y + 5.0), ListHit::TileButton(11, TileButton::Stop));
        // The bare shell's `▶ Start Claude` is the right end of its one line.
        let shell = &l.rows[5];
        let (b, start) = shell.actions[0];
        assert_eq!(b, TileButton::StartClaude);
        assert_eq!(start.right(), TR - TILE_PAD_H);
        assert_eq!(l.hit(start.x + 5.0, start.y + 5.0), ListHit::TileButton(30, TileButton::StartClaude));
        assert_eq!(ListHit::TileButton(30, TileButton::Stop).pane(), Some(30));
        assert_eq!(ListHit::HeaderAdd(2).group(), Some(2));
    }

    #[test]
    fn a_restored_session_offers_resume_where_a_bare_shell_offers_start_claude() {
        let mut m = model();
        let mut restored = tile(7, 0, TileState::Shell);
        restored.bare_shell = false;
        restored.resumable = true;
        m.groups = vec![group(0, false, vec![restored, tile(8, 0, TileState::Shell)])];
        let l = ListLayout::new(&m, W, &FakeMetrics);
        let (b, resume) = l.rows[1].actions[0];
        assert_eq!(b, TileButton::Resume);
        assert_eq!(resume.w, LINK_ICON_D + LINK_ICON_GAP + RESUME_LABEL.chars().count() as f64 * 6.0 + 8.0);
        assert_eq!(resume.right(), TR - TILE_PAD_H);
        assert_eq!(l.hit(resume.x + 5.0, resume.y + 5.0), ListHit::TileButton(7, TileButton::Resume));
        // The link is the only play: no play among the hover glyphs (as many
        // as leave the name TITLE_MIN_W, the leftmost giving way first).
        let all = [TileButton::Close, TileButton::Minimize, TileButton::MarkUnread];
        for r in [&l.rows[1], &l.rows[2]] {
            let got: Vec<TileButton> = r.glyphs.iter().map(|(b, _)| *b).collect();
            assert!(!got.is_empty() && got[..] == all[..got.len()], "{got:?}");
        }
        assert_eq!(l.rows[2].actions[0].0, TileButton::StartClaude);
        assert_eq!(l.rows[1].frame.h, l.rows[2].frame.h);
    }

    #[test]
    fn insertion_index_follows_the_midpoint_rule() {
        let l = ListLayout::new(&model(), W, &FakeMetrics);
        // Headers at 10..38, 114..142, 158..186; groups at 6, 110, 154.
        assert_eq!(l.insertion_index(10.0), 0);
        assert_eq!(l.insertion_index(100.0), 1);
        assert_eq!(l.insertion_index(130.0), 2);
        assert_eq!(l.insertion_index(200.0), 3);
        assert_eq!(l.insertion_line_y(0), 6.0 - GROUP_GAP / 2.0);
        assert_eq!(l.insertion_line_y(1), 110.0 - GROUP_GAP / 2.0);
        assert_eq!(l.insertion_line_y(3), 220.0 + GROUP_GAP / 2.0);
    }

    #[test]
    fn a_pane_drag_stays_inside_its_column_run() {
        let mut m = model();
        // Tab 0: column 0 holds panes 1, 2; column 1 holds pane 3. Tab 1: pane 4.
        m.groups = vec![
            group(0, false, vec![tile(1, 0, TileState::Idle), tile(2, 0, TileState::Idle), tile(3, 1, TileState::Idle)]),
            group(1, false, vec![tile(4, 0, TileState::Idle)]),
        ];
        let l = ListLayout::new(&m, W, &FakeMetrics);
        assert_eq!(l.pane_run(1), 1..3);
        assert_eq!(l.pane_run(2), 1..3);
        assert_eq!(l.pane_run(3), 3..4);
        assert_eq!(l.pane_run(5), 5..6);
        assert_eq!(l.pane_run(0), 0..0);
        // A row that is gone or no longer a tile (the list was re-laid out
        // mid-drag) has an empty run.
        assert_eq!(l.pane_run(99), 99..99);
        // Tile 1 spans 42..68, tile 2 72..98: the run's slots.
        assert_eq!(l.pane_slots(&(1..3)), vec![Slot { y: 42.0, h: 26.0 }, Slot { y: 72.0, h: 26.0 }]);
        assert_eq!(l.pane_slots(&(99..100)), Vec::<Slot>::new());
        // Lifting pane 1 and pulling it 17 down: its bottom (68 + 17) sits on
        // tile 2's middle (85), nothing crossed, the skeleton stays at its
        // origin; at 18 the bottom passes it: tile 2 steps up by 26 + 4 and
        // the skeleton takes its old top.
        let f = l.pane_drag_frame(1, 17.0, 0).unwrap();
        assert_eq!((f.run.clone(), f.from, f.to, f.dy), (1..3, 0, 0, 17.0));
        assert_eq!(f.next_to(17.0), 0);
        assert_eq!(f.next_to(18.0), 1);
        let f = l.pane_drag_frame(1, 18.0, 1).unwrap();
        assert_eq!(f.moves, vec![0.0, -30.0]);
        assert_eq!(f.placeholder_y, 72.0);
        // Hysteresis: tile 2 now sits 30 higher (its middle at 55), the held
        // one goes back under it only once its top (42 + dy) is above that.
        assert_eq!(f.next_to(13.0), 1);
        assert_eq!(f.next_to(12.0), 0);
        // The travel is clamped to the run: pane 1 cannot go above its own
        // top nor below tile 2's bottom; a stale rank is clamped too.
        let f = l.pane_drag_frame(1, -500.0, 5).unwrap();
        assert_eq!((f.dy, f.to), (0.0, 1));
        assert_eq!(l.pane_drag_frame(1, 500.0, 0).unwrap().dy, 30.0);
        // Pane 2 pulled up 18: its top (72 - 18) is above tile 1's middle
        // (55), so it aims at rank 0 and tile 1 steps down.
        let f = l.pane_drag_frame(2, -18.0, 1).unwrap();
        assert_eq!(f.next_to(-18.0), 0);
        let f = l.pane_drag_frame(2, -18.0, 0).unwrap();
        assert_eq!(f.moves, vec![30.0, 0.0]);
        assert_eq!(f.placeholder_y, 42.0);
        // A lone pane in its column has nowhere to go; a gone pane no frame.
        let f = l.pane_drag_frame(3, 30.0, 0).unwrap();
        assert_eq!((f.dy, f.to, f.moves.clone()), (0.0, 0, vec![0.0]));
        assert_eq!(l.pane_drag_frame(99, 0.0, 0), None);
        assert_eq!(l.row_for_pane(3), Some(3));
        assert_eq!(l.row_for_pane(99), None);
        assert_eq!(l.row_for_group(1), Some(4));
    }

    #[test]
    fn the_chrome_stacks_the_top_row_the_title_the_meta_row_and_the_list() {
        let c = ChromeLayout::new(W, 600.0, Some(40.0), 130.0);
        // Top row: four 26 pt buttons, 2 apart, 10 from the inner right
        // edge, on the traffic lights' own row.
        let switch_left = W - 1.0 - 10.0 - (4.0 * 26.0 + 3.0 * 2.0);
        assert_eq!(c.switch[0], Rect::new(switch_left, 6.0, 26.0, 26.0));
        assert_eq!(c.switch[1].x, c.switch[0].right() + 2.0);
        assert_eq!(c.switch[3].right(), W - 1.0 - 10.0);
        // Title row, 40 tall: the name 14 in, the pill 12 from the right,
        // 24 tall.
        assert_eq!(c.title, Rect::new(14.0, 38.0, W - 1.0 - 12.0 - 130.0 - 8.0 - 14.0, 40.0));
        assert_eq!(c.pill, Rect::new(W - 1.0 - 12.0 - 130.0, 46.0, 130.0, 24.0));
        // The title stops short of the pill: it truncates instead of colliding.
        assert_eq!(c.title.right() + 8.0, c.pill.x);
        // Meta row, 20 tall, 6 inside the list's inset: summary left, sort
        // ghost right.
        assert_eq!(c.summary, Rect::new(18.0, 78.0, W - 1.0 - 36.0, 20.0));
        assert_eq!(c.sort.right(), c.summary.right());
        assert_eq!(c.sort.w, 40.0 + 11.0 + 4.0 + 8.0);
        assert_eq!((c.sort.h, c.summary.bottom()), (16.0, 98.0));
        // No footer: the list runs to the bottom.
        assert_eq!(c.list, Rect::new(0.0, 98.0, W - 1.0 - 4.0, 600.0 - 98.0));

        // A press on a button switches lists; the rest of the top row still
        // drags the window.
        assert_eq!(c.hit(10.0, 10.0), ChromeHit::TopArea);
        for (rect, list) in c.switch.iter().zip(SWITCH_LISTS) {
            assert_eq!(c.hit(rect.x + 5.0, rect.y + 5.0), ChromeHit::ViewSwitch(list));
        }
        assert_eq!(c.hit(c.switch[0].right() + 1.0, 10.0), ChromeHit::TopArea);
        assert_eq!(c.hit(c.switch[0].x - 5.0, 10.0), ChromeHit::TopArea);
        // The title row is not a target; the pill is, on its own rect only.
        assert_eq!(c.hit(10.0, 50.0), ChromeHit::Empty);
        assert_eq!(c.hit(c.pill.x + 5.0, 50.0), ChromeHit::NextPill);
        assert_eq!(c.hit(c.pill.right() - 1.0, 50.0), ChromeHit::NextPill);
        assert_eq!(c.hit(c.pill.x - 5.0, 50.0), ChromeHit::Empty);
        // The sort ghost on the meta row, nothing else there.
        assert_eq!(c.hit(c.sort.x + 5.0, 85.0), ChromeHit::SortToggle);
        assert_eq!(c.hit(c.summary.x + 5.0, 85.0), ChromeHit::Empty);
        assert_eq!(c.hit(100.0, 300.0), ChromeHit::Empty);
        assert_eq!(c.hit(100.0, 585.0), ChromeHit::Empty);
        // The resize handle wins near the separator, on both sides.
        assert_eq!(c.hit(W - 4.0, 300.0), ChromeHit::Edge);
        assert_eq!(c.hit(W + 3.0, 300.0), ChromeHit::Edge);
        assert_eq!(c.hit(W - 6.0, 300.0), ChromeHit::Empty);
        assert_eq!(c.hit(W + 6.0, 300.0), ChromeHit::Empty);
    }

    #[test]
    fn the_chrome_fits_the_whole_width_range() {
        for w in [200.0, 300.0, 520.0] {
            let c = ChromeLayout::new(w, 600.0, Some(40.0), 130.0);
            let inner = w - 1.0;
            assert!(c.switch[0].x >= 0.0);
            assert_eq!(c.switch[3].right(), inner - 10.0);
            for (rect, list) in c.switch.iter().zip(SWITCH_LISTS) {
                assert_eq!(c.hit(rect.x + 13.0, rect.y + 13.0), ChromeHit::ViewSwitch(list));
            }
            // The title narrows before the pill does, and never overlaps it.
            assert!(c.title.w > 0.0);
            assert_eq!(c.title.right() + 8.0, c.pill.x);
            assert_eq!(c.pill.right(), inner - 12.0);
            assert!(c.sort.x > c.summary.x);
        }
        // A pill wider than the row gives way: it stops at the title's gap,
        // which is squeezed out rather than overlapped.
        let long = ChromeLayout::new(300.0, 600.0, Some(40.0), 400.0);
        assert_eq!(long.pill.x, 22.0);
        assert_eq!(long.pill.w, 300.0 - 1.0 - 12.0 - 22.0);
        assert_eq!(long.title.w, 0.0);
    }

    #[test]
    fn a_list_without_a_sort_control_has_no_ghost_and_no_hit() {
        let c = ChromeLayout::new(W, 600.0, None, 130.0);
        assert_eq!(c.sort, Rect::default());
        // The meta row keeps its text, and the whole row is the summary's.
        assert_eq!(c.hit(c.summary.right() - 5.0, 85.0), ChromeHit::Empty);
        assert_eq!(c.hit(c.summary.x + 5.0, 85.0), ChromeHit::Empty);
        // Everything else stays where it was.
        assert_eq!(c.list, ChromeLayout::new(W, 600.0, Some(40.0), 130.0).list);
    }

    #[test]
    fn the_agents_list_lays_its_tiles_and_empty_sections_out() {
        let mut m = model();
        m.view = SidebarList::Agents;
        let l = ListLayout::new(&m, 260.0, &FakeMetrics);
        // The empty list is still a layout: four stat tiles, the filtered
        // section's `None`, the empty `By tab`. It borrows no hint slot.
        assert!(!l.rows.is_empty());
        assert_eq!(l.agents.stats.len(), 4);
        assert!(l.groups.is_empty());
        assert_eq!(l.hint_y, None);
        assert_eq!(hint_text(&m), "No agents in this window");

        // Asks draws its own empty state, not the hint line.
        m.view = SidebarList::Asks;
        let asks = ListLayout::new(&m, 260.0, &FakeMetrics);
        assert_eq!(asks.hint_y, None);
        assert!(asks.asks.shown);
        assert!(asks.asks.empty.is_some());
        assert!(asks.asks.cards.is_empty());
    }

    #[test]
    fn the_next_pill_is_as_wide_as_its_content() {
        // Next, 3: pad 9, play 11 + gap 4, "Next" 4 chars x 6, gap 6, the
        // 16 pt circle, pad 9.
        assert_eq!(pill_width(NextPill::Next(3), &FakeMetrics), 9.0 + 15.0 + 24.0 + 6.0 + 16.0 + 9.0);
        // Two digits widen the circle: 12 + 8 = 20.
        assert_eq!(pill_width(NextPill::Next(12), &FakeMetrics), 9.0 + 15.0 + 24.0 + 6.0 + 20.0 + 9.0);
        // Nothing to read: no play, no badge; 15 chars.
        assert_eq!(pill_width(NextPill::Nothing, &FakeMetrics), 9.0 + 90.0 + 9.0);
    }

    #[test]
    fn styles_match_the_design_system_scale() {
        // 10 to 15 with the weights the design system pins, plus the Agents
        // stat tile's 20 pt value.
        assert_eq!(Style::ViewTitle.size(), 15.0);
        assert_eq!(Style::ViewTitle.weight(), Weight::Semibold);
        assert_eq!(Style::Title.size(), 13.0);
        assert_eq!(Style::Title.weight(), Weight::Medium);
        assert_eq!(Style::Secondary.size(), 12.0);
        assert_eq!(Style::Secondary.weight(), Weight::Regular);
        assert_eq!(Style::Link.size(), 12.0);
        assert_eq!(Style::Link.weight(), Weight::Medium);
        assert_eq!(Style::Pill.size(), 12.0);
        assert_eq!(Style::Pill.weight(), Weight::Semibold);
        assert_eq!(Style::Chip.size(), 11.0);
        assert_eq!(Style::Chip.weight(), Weight::Medium);
        assert_eq!(Style::Sort.size(), 11.0);
        assert_eq!(Style::Sort.weight(), Weight::Medium);
        assert_eq!(Style::PillBadge.size(), 10.0);
        assert_eq!(Style::PillBadge.weight(), Weight::Bold);
        // The filled button's label: the chip at 11, one step heavier.
        assert_eq!(Style::ChipStrong.size(), 11.0);
        assert_eq!(Style::ChipStrong.weight(), Weight::Semibold);
        assert_eq!(Style::StatValue.size(), 20.0);
        assert_eq!(Style::StatValue.weight(), Weight::Semibold);
        assert_eq!(Style::ALL.len(), 15);
        let _ = PaneFlags::default();
    }

    fn need(kind: NeedKind, n: u32) -> NeedRowVm {
        NeedRowVm {
            kind,
            key: format!("k{n}"),
            pane_id: Some(n),
            title: format!("pane {n}"),
            detail: "the detail".into(),
            age: "4m".into(),
        }
    }

    #[test]
    fn the_needs_block_sits_above_the_groups_and_folds_its_rows() {
        // Nothing waits: no card at all, and the groups keep their place.
        let mut m = model();
        let none = ListLayout::new(&m, W, &FakeMetrics);
        assert_eq!(none.needs_card, None);
        assert_eq!(none.rows[0].frame.y, LIST_TOP + GROUP_PAD_V);

        // Three asks: three rows, no button, the card above the first group.
        m.needs = (0..3).map(|i| need(NeedKind::Ask, i)).collect();
        let three = ListLayout::new(&m, W, &FakeMetrics);
        let card = three.needs_card.expect("a block with rows draws its card");
        assert_eq!(card, Rect::new(SX, LIST_TOP, SR - SX, NEED_PAD_TOP + NEED_HEADER_H + 3.0 * NEED_ROW_H + NEED_PAD_BOTTOM));
        assert_eq!(
            three.needs_header,
            Some(Rect::new(card.x + NEED_PAD_H, card.y + NEED_PAD_TOP, card.w - 2.0 * NEED_PAD_H, NEED_HEADER_H))
        );
        for (i, row) in three.rows.iter().take(3).enumerate() {
            assert_eq!(row.kind, RowKind::Need { index: i });
        }
        assert!(!three.rows.iter().any(|r| r.kind == RowKind::NeedAll));
        let header = three
            .rows
            .iter()
            .find(|r| r.kind == RowKind::Header { group: 0 })
            .expect("the groups follow the block");
        assert_eq!(header.frame.y, card.bottom() + NEED_MARGIN_B + GROUP_PAD_V);

        // `Show all` with nothing folded away changes nothing.
        m.needs_all = true;
        let three_all = ListLayout::new(&m, W, &FakeMetrics);
        assert_eq!(three_all.needs_card, three.needs_card);
        assert_eq!(three_all.rows.len(), three.rows.len());

        // Six asks: four rows and the button while folded.
        m.needs_all = false;
        m.needs = (0..6).map(|i| need(NeedKind::Ask, i)).collect();
        let six = ListLayout::new(&m, W, &FakeMetrics);
        assert_eq!(six.rows.iter().filter(|r| matches!(r.kind, RowKind::Need { .. })).count(), 4);
        assert!(six.rows.iter().any(|r| r.kind == RowKind::NeedAll));
        assert_eq!(
            six.needs_card.unwrap().h,
            NEED_PAD_TOP + NEED_HEADER_H + 4.0 * NEED_ROW_H + NEED_BUTTON_H + NEED_PAD_BOTTOM
        );

        // Shown: every row, and the button stays to fold them back.
        m.needs_all = true;
        let six_all = ListLayout::new(&m, W, &FakeMetrics);
        assert_eq!(six_all.rows.iter().filter(|r| matches!(r.kind, RowKind::Need { .. })).count(), 6);
        assert!(six_all.rows.iter().any(|r| r.kind == RowKind::NeedAll));
        assert_eq!(
            six_all.needs_card.unwrap().h,
            NEED_PAD_TOP + NEED_HEADER_H + 6.0 * NEED_ROW_H + NEED_BUTTON_H + NEED_PAD_BOTTOM
        );
        assert!(six_all.needs_card.unwrap().h > six.needs_card.unwrap().h);
    }

    #[test]
    fn the_block_keeps_every_error_and_folds_the_asks_to_four_rows() {
        let mut m = model();
        m.needs = vec![
            need(NeedKind::Error, 1),
            need(NeedKind::FailedRoutine, 2),
            need(NeedKind::Ask, 3),
            need(NeedKind::Ask, 4),
            need(NeedKind::Ask, 5),
            need(NeedKind::Ask, 6),
            need(NeedKind::Ask, 7),
        ];
        let l = ListLayout::new(&m, W, &FakeMetrics);
        let kinds: Vec<RowKind> =
            l.rows.iter().map(|r| r.kind).filter(|k| matches!(k, RowKind::Need { .. } | RowKind::NeedAll)).collect();
        // Two problems and two asks: four rows, then the button.
        assert_eq!(
            kinds,
            vec![
                RowKind::Need { index: 0 },
                RowKind::Need { index: 1 },
                RowKind::Need { index: 2 },
                RowKind::Need { index: 3 },
                RowKind::NeedAll
            ]
        );
        assert_eq!(need_all_label(7, false), "Show all 7");
        assert_eq!(need_all_label(7, true), "Show less");

        // More errors than rows: every one shows and nothing is folded.
        m.needs = (0..6).map(|i| need(NeedKind::Error, i)).collect();
        let errors = ListLayout::new(&m, W, &FakeMetrics);
        assert_eq!(errors.rows.iter().filter(|r| matches!(r.kind, RowKind::Need { .. })).count(), 6);
        assert!(!errors.rows.iter().any(|r| r.kind == RowKind::NeedAll));

        // The header counts: asks in warn, errors and failed runs in crit.
        let text: String = needs_summary(&m.needs).into_iter().map(|(t, _)| t).collect();
        assert_eq!(text, "6 errors");
        let mixed = vec![need(NeedKind::Ask, 1), need(NeedKind::Error, 2), need(NeedKind::FailedRoutine, 3)];
        let runs = needs_summary(&mixed);
        let text: String = runs.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text, "1 need you \u{b7} 1 error \u{b7} 1 failed");
        assert_eq!(runs[0].1, NeedTally::You);
        assert_eq!(runs[2].1, NeedTally::Problem);
        assert_eq!(runs[4].1, NeedTally::Problem);
    }

    #[test]
    fn a_narrow_block_drops_the_need_detail() {
        let mut m = model();
        m.needs = vec![need(NeedKind::Permission, 1)];
        let wide = ListLayout::new(&m, 320.0, &FakeMetrics);
        let narrow = ListLayout::new(&m, 260.0, &FakeMetrics);
        assert!(wide.rows[0].detail.w > 0.0);
        assert_eq!(narrow.rows[0].detail, Rect::default());
        // Same row height either way: only the detail column goes.
        assert_eq!(wide.rows[0].frame.h, narrow.rows[0].frame.h);
    }

    #[test]
    fn the_needs_rows_the_button_and_the_answer_link_hit_their_targets() {
        let mut m = model();
        m.needs = (1..=6).map(|i| need(NeedKind::Ask, i)).collect();
        m.groups[0].tiles[0].waiting = Some("4m".into());
        let l = ListLayout::new(&m, 320.0, &FakeMetrics);
        let row = l.rows.iter().find(|r| r.kind == RowKind::Need { index: 1 }).expect("the second need row");
        assert_eq!(l.hit(row.frame.x + 4.0, row.frame.y + 4.0), ListHit::Need(1));
        let button = l.rows.iter().find(|r| r.kind == RowKind::NeedAll).expect("the button");
        assert_eq!(l.hit(button.frame.x + 4.0, button.frame.y + 4.0), ListHit::NeedAll);
        // The waiting tile's right column ends on its `Answer` hit target.
        let tile = l.rows.iter().find(|r| matches!(r.kind, RowKind::Tile { pane_id: 10, .. })).expect("pane 10");
        let answer = tile.answer.expect("a waiting tile carries the link");
        assert!(tile.actions.is_empty(), "no Start Claude link next to the wait");
        // the one-line row has no subtitle left to carry it.
        assert_eq!(answer.right(), tile.frame.right() - TILE_PAD_H);
        let wait_w = "waiting 4m".len() as f64 * 6.0;
        let wait_x = answer.x - ANSWER_GAP - wait_w;
        assert!(wait_x > tile.frame.x + TEXT_X + 40.0, "the wait leaves the name its room");
        assert_eq!(tile.frame.h, TILE_HEIGHT);
        assert_eq!(l.hit(answer.x + 2.0, answer.y + 2.0), ListHit::NeedAnswer(10));
        // Left of the column the name line is still the tile.
        assert_eq!(l.hit(tile.frame.x + TEXT_X, answer.y + 2.0), ListHit::Tile(10));
        // A tile with nothing waiting has no link at all.
        assert_eq!(l.rows.iter().find(|r| matches!(r.kind, RowKind::Tile { pane_id: 11, .. })).and_then(|r| r.answer), None);
    }

    #[test]
    fn a_waiting_tile_never_lays_links_on_top_of_each_other() {
        // A waiting bare shell is the worst case: Start Claude would land on
        // the right end of the same line as the Answer link.
        let mut m = model();
        let mut waiting_shell = tile(30, 0, TileState::Shell);
        waiting_shell.waiting = Some("14s".into());
        m.groups[2].tiles = vec![waiting_shell];
        let l = ListLayout::new(&m, W, &FakeMetrics);
        let row = l.rows.iter().find(|r| matches!(r.kind, RowKind::Tile { pane_id: 30, .. })).expect("the shell");
        let answer = row.answer.expect("the waiting shell carries the link");
        assert!(row.actions.is_empty());
        // Starts after `waiting 14s`, not over it.
        let text_x = row.frame.x + TEXT_X;
        assert!(answer.x >= text_x + "waiting 14s".len() as f64 * 6.0);
        // And intersects no other link box of the list.
        let overlaps = |a: &Rect, b: &Rect| a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom();
        for r in &l.rows {
            for (_, link) in r.actions.iter().chain(r.glyphs.iter()) {
                assert!(!overlaps(&answer, link), "the Answer link overlaps a link at {link:?}");
            }
        }
    }
}
