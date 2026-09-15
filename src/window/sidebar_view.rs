//! The sidebar as an AppKit view: KovaLink's home screen in SF Pro, rounded
//! tiles and tinted chips, drawn with Cocoa (`drawRect:`) next to the Metal
//! terminal view. Two classes: `SidebarView` owns the chrome (traffic-light
//! strip, summary row, Next pill, footer, resize edge) and an `NSScrollView`;
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
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSCursor, NSEvent, NSFont, NSGraphicsContext, NSLineBreakMode, NSLineCapStyle,
    NSLineJoinStyle, NSMutableParagraphStyle, NSScrollElasticity, NSScrollView, NSScrollerStyle,
    NSStringDrawing, NSStringDrawingOptions, NSStringNSExtendedStringDrawing, NSTrackingArea,
    NSTrackingAreaOptions, NSView,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSDictionary, NSObjectProtocol, NSString};

use super::sidebar::{
    self, tab_tint, tokens, NextPill, SidebarSort, SummaryRun, TileButton, TileState, OPEN_LABEL,
    RESUME_LABEL, START_CLAUDE_LABEL, STOP_LABEL,
};
use super::sidebar_model::{GroupVm, SidebarModel, TileVm};
use super::sidebar_ui::{PaneAction, TabAction};
use super::KovaView;
use crate::config::LayoutMode;
use crate::pane::{PaneId, TabId};

// ---------------------------------------------------------------
// Metrics (points)
// ---------------------------------------------------------------

/// The separator line at the sidebar's right edge, part of its width.
pub const SEP_W: f64 = 1.0;
/// Points either side of the separator that grab the resize handle.
const EDGE_TOLERANCE: f64 = 4.0;
/// Traffic lights and the window drag region.
const TOP_H: f64 = 36.0;
const SUMMARY_H: f64 = 24.0;
const PILL_REGION_H: f64 = 36.0;
const PILL_H: f64 = 28.0;
const PILL_INSET: f64 = 12.0;
const FOOTER_H: f64 = 24.0;
/// Horizontal padding of the chrome and the list content.
const PAD_H: f64 = 12.0;
const LIST_TOP: f64 = 6.0;
const LIST_BOTTOM: f64 = 12.0;
const GROUP_GAP: f64 = 16.0;
const TILE_GAP: f64 = 6.0;
const HEADER_H: f64 = 28.0;
const HEADER_RADIUS: f64 = 6.0;
/// The tint bar at the left of a group, and the content inset after it.
const GROUP_BAR_W: f64 = 3.0;
const GROUP_CONTENT_X: f64 = 12.0;
/// The chevron zone at the left of a header.
const CHEVRON_ZONE_W: f64 = 24.0;
const ADD_D: f64 = 22.0;
const TILE_PAD_V: f64 = 8.0;
const TILE_PAD_H: f64 = 10.0;
const TILE_RADIUS: f64 = 10.0;
const CARD_PAD_V: f64 = 10.0;
const CARD_PAD_H: f64 = 12.0;
const CARD_RADIUS: f64 = 12.0;
const CARD_BAR_W: f64 = 4.0;
/// Line boxes inside a tile.
const LINE_TITLE_H: f64 = 20.0;
const LINE_SEC_H: f64 = 16.0;
const LINE_Q_H: f64 = 16.0;
const LINE_GAP: f64 = 2.0;
/// State glyph and text columns inside a tile.
const GLYPH_X: f64 = 10.0;
const GLYPH_D: f64 = 8.0;
const TEXT_X: f64 = 26.0;
const CHIP_H: f64 = 18.0;
const CHIP_PAD: f64 = 7.0;
const BUTTON_D: f64 = 20.0;
const BUTTON_GAP: f64 = 4.0;
const ACTIONS_H: f64 = 20.0;
const HINT_H: f64 = 20.0;
/// Pixels of travel before a pressed header or tile lifts into a drag.
const DRAG_THRESHOLD: f64 = 3.0;

// ---------------------------------------------------------------
// Text styles
// ---------------------------------------------------------------

/// The type scale, KovaLink's tokens two steps down for a Mac list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    Title,
    Secondary,
    Chip,
    Question,
    Detail,
    Summary,
    Sort,
    Pill,
    PillBadge,
    Link,
    Footer,
    Hint,
    Number,
    Plus,
    Glyph,
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
        Style::Title,
        Style::Secondary,
        Style::Chip,
        Style::Question,
        Style::Detail,
        Style::Summary,
        Style::Sort,
        Style::Pill,
        Style::PillBadge,
        Style::Link,
        Style::Footer,
        Style::Hint,
        Style::Number,
        Style::Plus,
        Style::Glyph,
    ];

    pub fn size(self) -> f64 {
        match self {
            Style::Title => 15.0,
            Style::Secondary | Style::Question => 13.0,
            Style::Detail | Style::Link | Style::Pill => 12.0,
            Style::Chip | Style::Summary | Style::Hint | Style::Number | Style::Glyph => 11.0,
            Style::Sort | Style::PillBadge | Style::Footer => 10.0,
            Style::Plus => 14.0,
        }
    }

    pub fn weight(self) -> Weight {
        match self {
            Style::Title | Style::Pill | Style::Link => Weight::Semibold,
            Style::Chip | Style::Sort | Style::Plus | Style::Glyph => Weight::Medium,
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
}

/// One laid-out row, in list (document) coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub kind: RowKind,
    pub frame: Rect,
    /// Tile: the hover glyph boxes on the title line, right to left.
    pub glyphs: Vec<(TileButton, Rect)>,
    /// Tile: the always-visible action buttons (Open, Stop, Start Claude).
    pub actions: Vec<(TileButton, Rect)>,
    /// Awaiting tile: lines the question takes (1 or 2).
    pub question_lines: usize,
}

/// What a point of the list lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListHit {
    /// The chevron zone of a header: fold or unfold, no tab switch.
    Chevron(usize),
    /// The rest of a header: switch to the tab (or fold the active one).
    Header(usize),
    /// The `+` of a header: add a pane.
    HeaderAdd(usize),
    Tile(PaneId),
    TileButton(PaneId, TileButton),
    Empty,
}

impl ListHit {
    pub fn pane(self) -> Option<PaneId> {
        match self {
            ListHit::Tile(id) | ListHit::TileButton(id, _) => Some(id),
            _ => None,
        }
    }

    pub fn group(self) -> Option<usize> {
        match self {
            ListHit::Chevron(g) | ListHit::Header(g) | ListHit::HeaderAdd(g) => Some(g),
            _ => None,
        }
    }
}

/// The list's geometry for one model at one width.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListLayout {
    pub width: f64,
    pub rows: Vec<Row>,
    /// One block per group (the tint bar spans it), parallel to `model.groups`.
    pub groups: Vec<Rect>,
    pub hint_y: Option<f64>,
    pub content_h: f64,
}

impl ListLayout {
    pub fn new(model: &SidebarModel, width: f64, m: &dyn TextMetrics) -> Self {
        let x0 = PAD_H;
        let x1 = (width - (PAD_H - EDGE_TOLERANCE)).max(x0 + 40.0);
        let content_x = x0 + GROUP_BAR_W + GROUP_CONTENT_X;
        let content_w = x1 - content_x;
        let mut rows = Vec::new();
        let mut groups = Vec::with_capacity(model.groups.len());
        let mut y = LIST_TOP;
        for (gi, g) in model.groups.iter().enumerate() {
            if gi > 0 {
                y += GROUP_GAP;
            }
            let top = y;
            rows.push(Row {
                kind: RowKind::Header { group: gi },
                frame: Rect::new(content_x, y, content_w, HEADER_H),
                glyphs: Vec::new(),
                actions: Vec::new(),
                question_lines: 0,
            });
            y += HEADER_H;
            for (ti, tile) in g.tiles.iter().enumerate() {
                y += TILE_GAP;
                let row = tile_row(gi, ti, tile, content_x, y, content_w, m);
                y += row.frame.h;
                rows.push(row);
            }
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
        ListLayout { width, rows, groups, hint_y, content_h: y }
    }

    /// The `+` button of a header row.
    pub fn add_button(frame: &Rect) -> Rect {
        Rect::new(frame.right() - 4.0 - ADD_D, frame.y + (HEADER_H - ADD_D) / 2.0, ADD_D, ADD_D)
    }

    pub fn hit(&self, x: f64, y: f64) -> ListHit {
        for row in &self.rows {
            if !row.frame.contains(x, y) {
                continue;
            }
            return match row.kind {
                RowKind::Header { group } => {
                    if x < row.frame.x + CHEVRON_ZONE_W {
                        ListHit::Chevron(group)
                    } else if Self::add_button(&row.frame).contains(x, y) {
                        ListHit::HeaderAdd(group)
                    } else {
                        ListHit::Header(group)
                    }
                }
                RowKind::Tile { pane_id, .. } => {
                    for (b, r) in row.glyphs.iter().chain(row.actions.iter()) {
                        if r.contains(x, y) {
                            return ListHit::TileButton(pane_id, *b);
                        }
                    }
                    ListHit::Tile(pane_id)
                }
            };
        }
        ListHit::Empty
    }

    pub fn row_for_pane(&self, pane_id: PaneId) -> Option<usize> {
        self.rows.iter().position(|r| matches!(r.kind, RowKind::Tile { pane_id: p, .. } if p == pane_id))
    }

    pub fn row_for_group(&self, group: usize) -> Option<usize> {
        self.rows.iter().position(|r| matches!(r.kind, RowKind::Header { group: g } if g == group))
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

    /// The slot (0 ..= run length) a dragged tile would take in `run`, by the
    /// midpoint rule, `None` when the cursor left the run vertically.
    pub fn pane_insertion_slot(&self, run: &std::ops::Range<usize>, y: f64) -> Option<usize> {
        if run.is_empty() || run.end > self.rows.len() {
            return None;
        }
        let first = &self.rows[run.start].frame;
        let last = &self.rows[run.end - 1].frame;
        if y < first.y - HEADER_H || y >= last.bottom() + HEADER_H {
            return None;
        }
        for (k, row) in self.rows[run.clone()].iter().enumerate() {
            if y < row.frame.y + row.frame.h / 2.0 {
                return Some(k);
            }
        }
        Some(run.len())
    }

    /// Y of the insertion line for `slot` in `run`.
    pub fn pane_insertion_line_y(&self, run: &std::ops::Range<usize>, slot: usize) -> f64 {
        if slot < run.len() {
            self.rows[run.start + slot].frame.y - TILE_GAP / 2.0
        } else {
            self.rows[run.end - 1].frame.bottom() + TILE_GAP / 2.0
        }
    }
}

/// Lay out one tile at `y`.
fn tile_row(group: usize, index: usize, tile: &TileVm, x: f64, y: f64, w: f64, m: &dyn TextMetrics) -> Row {
    let (pad_v, pad_h) = if tile.awaiting() { (CARD_PAD_V, CARD_PAD_H) } else { (TILE_PAD_V, TILE_PAD_H) };
    let text_x = x + TEXT_X;
    let right = x + w - pad_h;
    let text_w = (right - text_x).max(10.0);
    let line1_y = y + pad_v;
    let glyphs = TileButton::hover_glyphs(tile.state, tile.minimized, tile.bare_shell, tile.resumable)
        .into_iter()
        .enumerate()
        .map(|(k, b)| {
            let bx = right - (k as f64 + 1.0) * BUTTON_D - k as f64 * BUTTON_GAP;
            (b, Rect::new(bx, line1_y + (LINE_TITLE_H - BUTTON_D) / 2.0, BUTTON_D, BUTTON_D))
        })
        .collect();
    let mut actions = Vec::new();
    let mut question_lines = 0;
    let mut h = pad_v + LINE_TITLE_H;
    if tile.awaiting() {
        let question = tile.question.as_deref().unwrap_or("");
        question_lines = m.lines(question, Style::Question, text_w, 2).max(1);
        h += LINE_GAP + question_lines as f64 * LINE_Q_H;
        if tile.detail.is_some() {
            h += LINE_GAP + LINE_SEC_H;
        }
        h += 4.0;
        let open_w = m.width(OPEN_LABEL, Style::Link) + 20.0;
        actions.push((TileButton::Open, Rect::new(text_x, y + h, open_w, ACTIONS_H)));
        let stop_w = m.width(STOP_LABEL, Style::Link) + 8.0;
        actions.push((TileButton::Stop, Rect::new(right - stop_w, y + h, stop_w, ACTIONS_H)));
        h += ACTIONS_H;
    } else {
        h += LINE_GAP + LINE_SEC_H;
        let call = if tile.bare_shell {
            Some((TileButton::StartClaude, START_CLAUDE_LABEL))
        } else if tile.resumable {
            Some((TileButton::Resume, RESUME_LABEL))
        } else {
            None
        };
        if let Some((button, label)) = call {
            let call_w = m.width(label, Style::Link) + 8.0;
            let line2_y = line1_y + LINE_TITLE_H + LINE_GAP;
            actions.push((button, Rect::new(right - call_w, line2_y - 1.0, call_w, LINE_SEC_H + 2.0)));
        }
        if tile.summary.is_some() {
            h += LINE_GAP + LINE_SEC_H;
        }
    }
    h += pad_v;
    Row {
        kind: RowKind::Tile { group, index, pane_id: tile.pane_id, column: tile.column },
        frame: Rect::new(x, y, w, h),
        glyphs,
        actions,
        question_lines,
    }
}

/// What a point of the chrome lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeHit {
    /// The traffic-light strip: window drag region.
    TopArea,
    SortToggle,
    NextPill,
    ModeButton,
    /// The resize handle at the right edge.
    Edge,
    Empty,
}

/// The fixed parts of the sidebar, in the sidebar view's flipped coordinates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChromeLayout {
    pub width: f64,
    pub height: f64,
    pub summary: Rect,
    pub sort: Rect,
    pub pill: Rect,
    pub list: Rect,
    pub footer: Rect,
    pub mode_button: Rect,
}

impl ChromeLayout {
    pub fn new(width: f64, height: f64, sort_label_w: f64, mode_label_w: f64) -> Self {
        let inner_w = width - SEP_W;
        let summary = Rect::new(PAD_H, TOP_H, inner_w - 2.0 * PAD_H, SUMMARY_H);
        let sort_w = sort_label_w + 12.0;
        let sort = Rect::new(summary.right() - sort_w, TOP_H + (SUMMARY_H - 20.0) / 2.0, sort_w, 20.0);
        let pill = Rect::new(PILL_INSET, TOP_H + SUMMARY_H + (PILL_REGION_H - PILL_H) / 2.0, inner_w - 2.0 * PILL_INSET, PILL_H);
        let list_y = TOP_H + SUMMARY_H + PILL_REGION_H;
        let list = Rect::new(0.0, list_y, inner_w - EDGE_TOLERANCE, (height - list_y - FOOTER_H).max(0.0));
        let footer = Rect::new(0.0, height - FOOTER_H, inner_w, FOOTER_H);
        let mode_w = mode_label_w + 12.0;
        let mode_button = Rect::new(inner_w - PAD_H - mode_w, footer.y + (FOOTER_H - 20.0) / 2.0, mode_w, 20.0);
        ChromeLayout { width, height, summary, sort, pill, list, footer, mode_button }
    }

    pub fn hit(&self, x: f64, y: f64) -> ChromeHit {
        if (x - (self.width - SEP_W / 2.0)).abs() <= EDGE_TOLERANCE {
            return ChromeHit::Edge;
        }
        if x < 0.0 || x >= self.width || y < 0.0 || y >= self.height {
            return ChromeHit::Empty;
        }
        if y < TOP_H {
            return ChromeHit::TopArea;
        }
        if self.sort.contains(x, y) {
            return ChromeHit::SortToggle;
        }
        if self.pill.contains(x, y) {
            return ChromeHit::NextPill;
        }
        if self.mode_button.contains(x, y) {
            return ChromeHit::ModeButton;
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
#[derive(Clone, Copy)]
struct PaneDrag {
    pane_id: PaneId,
    start_y: f64,
    current_y: f64,
    grab_offset: f64,
    dragging: bool,
}

struct Shared {
    model: SidebarModel,
    layout: ListLayout,
    chrome: ChromeLayout,
    fonts: Vec<(Style, Retained<NSFont>)>,
    hovered: ListHit,
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
struct AppKitMetrics<'a>(&'a Shared);

impl TextMetrics for AppKitMetrics<'_> {
    fn width(&self, text: &str, style: Style) -> f64 {
        let attrs = attrs(self.0.font(style), tokens::TEXT_PRIMARY, 1.0, NSLineBreakMode::ByClipping);
        let size = unsafe { NSString::from_str(text).sizeWithAttributes(Some(&attrs)) };
        size.width.ceil()
    }

    fn lines(&self, text: &str, style: Style, width: f64, max: usize) -> usize {
        if text.is_empty() {
            return 1;
        }
        let attrs = attrs(self.0.font(style), tokens::TEXT_PRIMARY, 1.0, NSLineBreakMode::ByWordWrapping);
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

fn fill_round(r: &Rect, radius: f64, c: [f32; 3], alpha: f64) {
    color(c, alpha).setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r.cg(), radius, radius).fill();
}

fn stroke_round(r: &Rect, radius: f64, width: f64, c: [f32; 3], alpha: f64) {
    color(c, alpha).setStroke();
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r.inset(width / 2.0).cg(), radius, radius);
    path.setLineWidth(width);
    path.stroke();
}

fn fill_rect(r: &Rect, c: [f32; 3], alpha: f64) {
    color(c, alpha).setFill();
    NSBezierPath::bezierPathWithRect(r.cg()).fill();
}

fn fill_dot(cx: f64, cy: f64, d: f64, c: [f32; 3], alpha: f64) {
    color(c, alpha).setFill();
    NSBezierPath::bezierPathWithOvalInRect(Rect::new(cx - d / 2.0, cy - d / 2.0, d, d).cg()).fill();
}

fn stroke_ring(cx: f64, cy: f64, d: f64, width: f64, c: [f32; 3], alpha: f64) {
    color(c, alpha).setStroke();
    let path = NSBezierPath::bezierPathWithOvalInRect(Rect::new(cx - d / 2.0, cy - d / 2.0, d, d).inset(width / 2.0).cg());
    path.setLineWidth(width);
    path.stroke();
}

/// A chevron pointing down (open) or right (folded), 8 pt wide.
fn stroke_chevron(x: f64, cy: f64, open: bool, c: [f32; 3], alpha: f64) {
    color(c, alpha).setStroke();
    let path = NSBezierPath::bezierPath();
    path.setLineWidth(1.5);
    path.setLineCapStyle(NSLineCapStyle::Round);
    path.setLineJoinStyle(NSLineJoinStyle::Round);
    if open {
        path.moveToPoint(CGPoint { x, y: cy - 2.0 });
        path.lineToPoint(CGPoint { x: x + 4.0, y: cy + 2.0 });
        path.lineToPoint(CGPoint { x: x + 8.0, y: cy - 2.0 });
    } else {
        path.moveToPoint(CGPoint { x: x + 2.0, y: cy - 4.0 });
        path.lineToPoint(CGPoint { x: x + 6.0, y: cy });
        path.lineToPoint(CGPoint { x: x + 2.0, y: cy + 4.0 });
    }
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
enum Align {
    Left,
    Right,
    Center,
}

/// Draw one line of text vertically centred in `r`, truncated with an
/// ellipsis (at the tail; at the head for rename edit buffers, so the cursor
/// stays visible). Returns the drawn width.
fn draw_text(sh: &Shared, text: &str, r: &Rect, style: Style, c: [f32; 3], alpha: f64, align: Align, head: bool) -> f64 {
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
    // Symbol glyphs (+, x, stop, play) sit off centre when their line box is
    // centred, so centre their ink on the box instead.
    if matches!(style, Style::Plus | Style::Glyph) {
        // Device metrics give the ink box relative to the text origin.
        let ink = unsafe {
            ns.boundingRectWithSize_options_attributes_context(
                CGSize { width: 10_000.0, height: 10_000.0 },
                NSStringDrawingOptions::UsesLineFragmentOrigin | NSStringDrawingOptions::UsesDeviceMetrics,
                Some(&a),
                None,
            )
        };
        let ox = r.x + r.w / 2.0 - (ink.origin.x + ink.size.width / 2.0);
        let oy = r.y + r.h / 2.0 - (ink.origin.y + ink.size.height / 2.0);
        unsafe { ns.drawAtPoint_withAttributes(CGPoint { x: ox, y: oy }, Some(&a)) };
        return w;
    }
    let y = r.y + ((r.h - size.height) / 2.0).round();
    unsafe { ns.drawInRect_withAttributes(Rect::new(x, y, w, size.height).cg(), Some(&a)) };
    w
}

/// Draw a wrapped text of at most `lines` lines in `r`, the last line ending
/// with an ellipsis when cut.
fn draw_wrapped(sh: &Shared, text: &str, r: &Rect, style: Style, c: [f32; 3], alpha: f64) {
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

/// A chip: rounded fill and its caption. Returns its rect.
fn draw_chip(sh: &Shared, text: &str, right: f64, cy: f64, bg: [f32; 3], fg: [f32; 3], dot: Option<[f32; 3]>, alpha: f64) -> Rect {
    let text_w = AppKitMetrics(sh).width(text, Style::Chip);
    let dot_w = if dot.is_some() { GLYPH_D - 2.0 + 5.0 } else { 0.0 };
    let w = text_w + 2.0 * CHIP_PAD + dot_w;
    let r = Rect::new(right - w, cy - CHIP_H / 2.0, w, CHIP_H);
    fill_round(&r, CHIP_H / 2.0, bg, alpha);
    if let Some(d) = dot {
        fill_dot(r.x + CHIP_PAD + 3.0, cy, 6.0, d, alpha);
    }
    draw_text(sh, text, &Rect::new(r.x + CHIP_PAD + dot_w, r.y, text_w, CHIP_H), Style::Chip, fg, alpha, Align::Left, false);
    r
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
    Style::ALL
        .iter()
        .map(|&s| {
            let weight = unsafe {
                match s.weight() {
                    Weight::Regular => objc2_app_kit::NSFontWeightRegular,
                    Weight::Medium => objc2_app_kit::NSFontWeightMedium,
                    Weight::Semibold => objc2_app_kit::NSFontWeightSemibold,
                    Weight::Bold => objc2_app_kit::NSFontWeightBold,
                }
            };
            (s, NSFont::systemFontOfSize_weight(s.size(), weight))
        })
        .collect()
}

// ---------------------------------------------------------------
// SidebarView: chrome and scroll view
// ---------------------------------------------------------------

pub struct SidebarViewIvars {
    shared: Rc<RefCell<Shared>>,
    scroll: OnceCell<Retained<NSScrollView>>,
    list: OnceCell<Retained<SidebarListView>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "KovaSidebarView"]
    #[ivars = SidebarViewIvars]
    pub struct SidebarView;

    unsafe impl NSObjectProtocol for SidebarView {}

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
                ChromeHit::SortToggle | ChromeHit::NextPill | ChromeHit::ModeButton => hit,
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
                ChromeHit::SortToggle | ChromeHit::NextPill | ChromeHit::ModeButton => {
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
                            ChromeHit::NextPill => kova.sidebar_next_pill_clicked(),
                            ChromeHit::ModeButton => kova.set_layout_mode(LayoutMode::Tabs),
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
    pub fn new(mtm: MainThreadMarker, frame: CGRect) -> Retained<Self> {
        let shared = Rc::new(RefCell::new(Shared {
            model: SidebarModel {
                summary: Vec::new(),
                sort: SidebarSort::Kova,
                pill: NextPill::Nothing,
                groups: Vec::new(),
                show_hint: false,
            },
            layout: ListLayout::default(),
            chrome: ChromeLayout::default(),
            fonts: build_fonts(),
            hovered: ListHit::Empty,
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
        let sort_changed = {
            let mut sh = self.ivars().shared.borrow_mut();
            let changed = sh.model.sort != model.sort;
            sh.model = model;
            changed
        };
        // A new sort label changes the chrome (and, through it, the list);
        // anything else only moves the rows.
        if sort_changed {
            self.layout_chrome();
        } else {
            self.relayout_list();
            self.setNeedsDisplay(true);
        }
        true
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
            list.scrollRectToVisible(Rect::new(f.x, f.y - TILE_GAP, f.w, f.h + 2.0 * TILE_GAP).cg());
        }
    }

    /// Position the scroll view for the current size, and re-lay the list.
    fn layout_chrome(&self) {
        let b = self.bounds();
        let (sort_w, mode_w) = {
            let sh = self.ivars().shared.borrow();
            let m = AppKitMetrics(&sh);
            (m.width(sh.model.sort.label(), Style::Sort), m.width(MODE_LABEL, Style::Link))
        };
        let chrome = ChromeLayout::new(b.size.width, b.size.height, sort_w, mode_w);
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
            }
        }
        let visible_h = self.ivars().shared.borrow().chrome.list.h;
        list.setFrameSize(CGSize { width, height: content_h.max(visible_h) });
        list.install_tooltips();
        list.setNeedsDisplay(true);
    }

    fn draw_chrome(&self) {
        let sh = self.ivars().shared.borrow();
        let c = &sh.chrome;
        let b = self.bounds();
        // Ground and separator.
        fill_rect(&Rect::new(0.0, 0.0, b.size.width, b.size.height), tokens::GROUND, 1.0);
        fill_rect(&Rect::new(b.size.width - SEP_W, 0.0, SEP_W, b.size.height), tokens::SEPARATOR, 1.0);

        // Summary row: coloured counts on the left, sort toggle on the right.
        let sort_hovered = sh.chrome_hovered == ChromeHit::SortToggle;
        let sort_pressed = sh.chrome_pressed == ChromeHit::SortToggle;
        if sort_hovered || sort_pressed {
            fill_round(&c.sort, 6.0, tokens::TILE_PRESSED, if sort_pressed { 1.0 } else { 0.7 });
        }
        let sort_fg = if sort_hovered {
            tokens::TEXT_PRIMARY
        } else if sh.model.sort == SidebarSort::Activity {
            tokens::ACCENT
        } else {
            tokens::TEXT_TERTIARY
        };
        draw_text(&sh, sh.model.sort.label(), &c.sort.inset(0.0), Style::Sort, sort_fg, 1.0, Align::Center, false);
        let mut x = c.summary.x;
        let max_x = c.sort.x - 8.0;
        for (text, run) in &sh.model.summary {
            let fg = match run {
                SummaryRun::Waiting => tokens::AWAITING,
                SummaryRun::Working => tokens::WORKING,
                SummaryRun::Idle | SummaryRun::Dot => tokens::TEXT_TERTIARY,
            };
            if x >= max_x {
                break;
            }
            let w = draw_text(&sh, text, &Rect::new(x, c.summary.y, max_x - x, c.summary.h), Style::Summary, fg, 1.0, Align::Left, false);
            x += w;
        }

        // Next pill.
        {
            let hovered = sh.chrome_hovered == ChromeHit::NextPill;
            let pressed = sh.chrome_pressed == ChromeHit::NextPill;
            let (fill, border, text, badge): ([f32; 3], Option<[f32; 3]>, [f32; 3], Option<([f32; 3], [f32; 3])>) = match sh.model.pill {
                NextPill::Next(_) => (
                    if hovered { tokens::ACCENT_PRESSED } else { tokens::ACCENT },
                    None,
                    tokens::TEXT_ON_FILL,
                    Some((tokens::TEXT_ON_FILL, tokens::ACCENT)),
                ),
                NextPill::Idle(_) => (
                    if hovered { tokens::TILE_PRESSED } else { tokens::TILE_HOVER },
                    Some(tokens::BORDER_STRONG),
                    tokens::TEXT_SECONDARY,
                    Some((tokens::TILE_PRESSED, tokens::TEXT_SECONDARY)),
                ),
                NextPill::CaughtUp => (tokens::TILE_HOVER, None, tokens::SUCCESS, None),
                NextPill::Nothing => (tokens::TILE_HOVER, None, tokens::TEXT_TERTIARY, None),
            };
            let text_alpha = if pressed { 0.85 } else { 1.0 };
            fill_round(&c.pill, PILL_H / 2.0, fill, 1.0);
            if let Some(b) = border {
                stroke_round(&c.pill, PILL_H / 2.0, 1.0, b, 1.0);
            }
            let mut right = c.pill.right() - 12.0;
            let hint_w = draw_text(&sh, "\u{2318}J", &Rect::new(c.pill.x, c.pill.y, right - c.pill.x, c.pill.h), Style::Sort, text, 0.6 * text_alpha, Align::Right, false);
            right -= hint_w + 8.0;
            if let (Some(n), Some((bg, fg))) = (sh.model.pill.badge(), badge) {
                let label = n.to_string();
                let w = (AppKitMetrics(&sh).width(&label, Style::PillBadge) + 8.0).max(18.0);
                let r = Rect::new(right - w, c.pill.y + (PILL_H - 18.0) / 2.0, w, 18.0);
                fill_round(&r, 9.0, bg, text_alpha);
                draw_text(&sh, &label, &r, Style::PillBadge, fg, 1.0, Align::Center, false);
                right -= w + 8.0;
            }
            draw_text(&sh, sh.model.pill.label(), &Rect::new(c.pill.x + 12.0, c.pill.y, right - c.pill.x - 12.0, c.pill.h), Style::Pill, text, text_alpha, Align::Left, false);
        }

        // Footer: version and the way back to the tab bar.
        fill_rect(&Rect::new(0.0, c.footer.y, c.footer.w, 1.0), tokens::BORDER_SUBTLE, 1.0);
        let version = format!("Kova v{}", env!("CARGO_PKG_VERSION"));
        draw_text(&sh, &version, &Rect::new(PAD_H, c.footer.y, c.mode_button.x - PAD_H - 8.0, c.footer.h), Style::Footer, tokens::TEXT_TERTIARY, 1.0, Align::Left, false);
        let mode_hovered = sh.chrome_hovered == ChromeHit::ModeButton;
        let mode_pressed = sh.chrome_pressed == ChromeHit::ModeButton;
        if mode_hovered || mode_pressed {
            fill_round(&c.mode_button, 6.0, tokens::TILE_PRESSED, if mode_pressed { 1.0 } else { 0.7 });
        }
        let mode_fg = if mode_hovered { tokens::TEXT_PRIMARY } else { tokens::TEXT_SECONDARY };
        draw_text(&sh, MODE_LABEL, &c.mode_button, Style::Link, mode_fg, 1.0, Align::Center, false);
    }
}

/// The footer button back to the tab bar.
const MODE_LABEL: &str = "\u{ab} Tab bar";

// ---------------------------------------------------------------
// SidebarListView: groups and tiles
// ---------------------------------------------------------------

pub struct SidebarListViewIvars {
    shared: Rc<RefCell<Shared>>,
    /// The mouse went down inside the list: the matching mouse up is ours
    /// whatever it lands on.
    mouse_down: std::cell::Cell<bool>,
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

        /// `NSToolTipOwner`: the tooltip of the glyph button under the mouse.
        #[unsafe(method_id(view:stringForToolTip:point:userData:))]
        #[unsafe(method_family = none)]
        unsafe fn tooltip_string(&self, _view: &NSView, _tag: isize, point: CGPoint, _data: *mut std::ffi::c_void) -> Retained<NSString> {
            let text = match self.ivars().shared.borrow().layout.hit(point.x, point.y) {
                ListHit::TileButton(_, b) => b.tooltip(),
                _ => "",
            };
            NSString::from_str(text)
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
                            let row_y = sh.layout.rows[row].frame.y;
                            sh.pane_drag = Some(PaneDrag { pane_id, start_y: y, current_y: y, grab_offset: y - row_y, dragging: false });
                        }
                    }
                }
                ListHit::Chevron(_) | ListHit::HeaderAdd(_) | ListHit::TileButton(..) => {
                    self.ivars().shared.borrow_mut().pressed = hit;
                }
                ListHit::Empty => {}
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
                    }
                    if d.dragging {
                        sh.pressed = ListHit::Empty;
                        lifted = true;
                    }
                    sh.pane_drag = Some(d);
                } else if sh.pressed != ListHit::Empty {
                    let hit = sh.layout.hit(x, y);
                    if hit != sh.pressed {
                        sh.pressed = ListHit::Empty;
                    }
                }
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
            if let Some(d) = tab_drag.filter(|d| d.dragging) {
                self.drop_tab(d.tab_id, y);
            } else if let Some(d) = pane_drag.filter(|d| d.dragging) {
                self.drop_pane(d.pane_id, y);
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
            let hit = self.ivars().shared.borrow().layout.hit(x, y);
            let Some(kova) = kova_of(self) else { return };
            if let Some(pane_id) = hit.pane() {
                kova.show_sidebar_pane_menu(self, CGPoint { x, y }, pane_id);
            } else if let Some(g) = hit.group() {
                let tab_idx = self.ivars().shared.borrow().model.groups.get(g).map(|g| g.tab_idx);
                if let Some(tab_idx) = tab_idx {
                    kova.show_sidebar_tab_menu(self, CGPoint { x, y }, tab_idx);
                }
            }
        }
    }
);

impl SidebarListView {
    fn new(mtm: MainThreadMarker, shared: Rc<RefCell<Shared>>, frame: CGRect) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(SidebarListViewIvars { shared, mouse_down: std::cell::Cell::new(false) });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn set_hovered(&self, hit: ListHit) {
        let mut sh = self.ivars().shared.borrow_mut();
        if sh.hovered != hit {
            sh.hovered = hit;
            drop(sh);
            self.setNeedsDisplay(true);
        }
    }

    /// One tooltip zone per glyph button, answered by `tooltip_string`.
    fn install_tooltips(&self) {
        // Collect first: the tooltip manager may call back into this view.
        let rects: Vec<CGRect> = {
            let sh = self.ivars().shared.borrow();
            sh.layout.rows.iter().flat_map(|row| row.glyphs.iter().map(|(_, r)| r.cg())).collect()
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
            ListHit::Chevron(g) => {
                if let Some(t) = tab_of(g) {
                    kova.sidebar_toggle_collapsed(t);
                }
            }
            ListHit::Header(g) => {
                if let Some(t) = tab_of(g) {
                    kova.sidebar_header_clicked(t);
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
                    TileButton::Open => PaneAction::Open,
                };
                kova.dispatch_pane_action(pane_id, action);
            }
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

    /// Drop a dragged tile at the slot under `y` in its column run. The pane
    /// may have gone since the mouse went down: then nothing happens.
    fn drop_pane(&self, pane_id: PaneId, y: f64) {
        let (tab_idx, ids, from, to) = {
            let sh = self.ivars().shared.borrow();
            let Some(row) = sh.layout.row_for_pane(pane_id) else { return };
            let run = sh.layout.pane_run(row);
            let Some(slot) = sh.layout.pane_insertion_slot(&run, y) else { return };
            let RowKind::Tile { group, .. } = sh.layout.rows[row].kind else { return };
            let Some(tab_idx) = sh.model.groups.get(group).map(|g| g.tab_idx) else { return };
            let ids: Vec<PaneId> = sh.layout.rows[run.clone()]
                .iter()
                .filter_map(|r| match r.kind {
                    RowKind::Tile { pane_id, .. } => Some(pane_id),
                    _ => None,
                })
                .collect();
            let from = row - run.start;
            (tab_idx, ids, from, sidebar::drop_index(from, slot))
        };
        if let Some(kova) = kova_of(self) {
            kova.sidebar_drop_pane(tab_idx, &ids, from, to);
        }
    }

    fn draw_list(&self, dirty: CGRect) {
        let sh = self.ivars().shared.borrow();
        let dirty_top = dirty.origin.y;
        let dirty_bottom = dirty.origin.y + dirty.size.height;
        let visible = |r: &Rect| r.bottom() >= dirty_top && r.y <= dirty_bottom;

        // Group blocks: the tint bar down the left, clipped to the block's
        // rounded corners.
        for (gi, block) in sh.layout.groups.iter().enumerate() {
            if !visible(block) {
                continue;
            }
            let Some(g) = sh.model.groups.get(gi) else { continue };
            NSGraphicsContext::saveGraphicsState_class();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(block.cg(), HEADER_RADIUS, HEADER_RADIUS).addClip();
            fill_rect(&Rect::new(block.x, block.y, GROUP_BAR_W, block.h), tab_tint(g.color), if g.active { 1.0 } else { 0.55 });
            NSGraphicsContext::restoreGraphicsState_class();
        }

        // The lifted header or tile, resolved against the current layout: a
        // tick can re-lay the list out while the mouse is down.
        let lifted_tab = sh.tab_drag.filter(|d| d.dragging);
        let lifted_group = lifted_tab.and_then(|d| sh.model.groups.iter().position(|g| g.tab_id == d.tab_id));
        let lifted_pane = sh.pane_drag.filter(|d| d.dragging);
        let lifted_row = lifted_pane.and_then(|d| sh.layout.row_for_pane(d.pane_id));
        for (ri, row) in sh.layout.rows.iter().enumerate() {
            if !visible(&row.frame) {
                continue;
            }
            let lifted = match row.kind {
                RowKind::Header { group } => lifted_group == Some(group),
                RowKind::Tile { .. } => lifted_row == Some(ri),
            };
            let alpha = if lifted { 0.35 } else { 1.0 };
            self.draw_row(&sh, row, row.frame.y, alpha);
        }

        if let Some(hy) = sh.layout.hint_y {
            let r = Rect::new(PAD_H, hy, sh.layout.width - 2.0 * PAD_H, HINT_H);
            draw_text(&sh, "\u{2318}T new tab \u{b7} \u{2318}D split", &r, Style::Hint, tokens::TEXT_TERTIARY, 1.0, Align::Center, false);
        }

        // Drag feedback: the insertion line, then the floating copy.
        if let (Some(d), Some(g)) = (lifted_tab, lifted_group) {
            let k = sh.layout.insertion_index(d.current_y);
            let ly = sh.layout.insertion_line_y(k);
            fill_round(&Rect::new(PAD_H, ly - 1.0, sh.layout.width - 2.0 * PAD_H - (PAD_H - EDGE_TOLERANCE), 2.0), 1.0, tokens::ACCENT, 1.0);
            if let Some(r) = sh.layout.row_for_group(g) {
                let row = &sh.layout.rows[r];
                self.draw_row(&sh, row, d.current_y - d.grab_offset, 0.9);
            }
        }
        if let (Some(d), Some(ri)) = (lifted_pane, lifted_row) {
            let run = sh.layout.pane_run(ri);
            if let Some(slot) = sh.layout.pane_insertion_slot(&run, d.current_y) {
                let ly = sh.layout.pane_insertion_line_y(&run, slot);
                let f = &sh.layout.rows[ri].frame;
                fill_round(&Rect::new(f.x, ly - 1.0, f.w, 2.0), 1.0, tokens::ACCENT, 1.0);
            }
            let row = &sh.layout.rows[ri];
            self.draw_row(&sh, row, d.current_y - d.grab_offset, 0.9);
        }
    }

    /// Draw one row with its top at `y` (its own y, or the cursor's while
    /// dragged).
    fn draw_row(&self, sh: &Shared, row: &Row, y: f64, alpha: f64) {
        let dy = y - row.frame.y;
        let frame = Rect::new(row.frame.x, y, row.frame.w, row.frame.h);
        match row.kind {
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
        }
    }

    fn draw_header(&self, sh: &Shared, g: &GroupVm, group: usize, frame: &Rect, alpha: f64) {
        let tint = tab_tint(g.color);
        let hovered = matches!(sh.hovered, ListHit::Header(h) | ListHit::Chevron(h) if h == group);
        let pressed = matches!(sh.pressed, ListHit::Header(h) | ListHit::Chevron(h) if h == group);
        // Ground: the active tab wears its tint, the others only light up
        // under the mouse.
        if g.active {
            if g.color.is_some() {
                fill_round(frame, HEADER_RADIUS, tint, 0.15 * alpha);
            } else {
                fill_round(frame, HEADER_RADIUS, tokens::TILE_HOVER, alpha);
            }
        }
        if pressed {
            fill_round(frame, HEADER_RADIUS, tokens::TEXT_ON_FILL, 0.12 * alpha);
        } else if hovered {
            fill_round(frame, HEADER_RADIUS, tokens::TEXT_ON_FILL, 0.06 * alpha);
        }
        let cy = frame.y + frame.h / 2.0;
        stroke_chevron(frame.x + 8.0, cy, !g.collapsed, tokens::TEXT_SECONDARY, alpha);
        fill_dot(frame.x + CHEVRON_ZONE_W + 6.0, cy, GLYPH_D, tint, alpha);
        let mut x = frame.x + CHEVRON_ZONE_W + 16.0;
        let number = (g.tab_idx + 1).to_string();
        let nw = draw_text(sh, &number, &Rect::new(x, frame.y, 30.0, frame.h), Style::Number, tokens::TEXT_TERTIARY, alpha, Align::Left, false);
        x += nw + 5.0;

        // Right side: the `+`, then the collapsed summary.
        let add = ListLayout::add_button(frame);
        let add_hovered = sh.hovered == ListHit::HeaderAdd(group);
        let add_pressed = sh.pressed == ListHit::HeaderAdd(group);
        fill_round(&add, ADD_D / 2.0, if add_hovered || add_pressed { tokens::TILE_PRESSED } else { tokens::TILE }, alpha);
        draw_text(sh, "+", &add, Style::Plus, if add_hovered { tokens::TEXT_PRIMARY } else { tokens::TEXT_SECONDARY }, alpha, Align::Center, false);
        let mut right = add.x - 8.0;
        if g.collapsed {
            let label = g.summary.count_label();
            let w = draw_text(sh, &label, &Rect::new(x, frame.y, right - x, frame.h), Style::Chip, tokens::TEXT_TERTIARY, alpha, Align::Right, false);
            right -= w + 6.0;
            if g.summary.working > 0 {
                fill_dot(right - 3.0, cy, 6.0, tokens::WORKING, alpha);
                right -= 10.0;
            }
            if g.summary.awaiting > 0 {
                fill_dot(right - 3.0, cy, 6.0, tokens::AWAITING, alpha);
                right -= 10.0;
            }
        }
        let title_fg = if g.active && g.color.is_some() { tint } else { tokens::TEXT_PRIMARY };
        draw_text(sh, &g.title, &Rect::new(x, frame.y, (right - x).max(0.0), frame.h), Style::Title, title_fg, alpha, Align::Left, g.renaming);
    }

    fn draw_tile(&self, sh: &Shared, tile: &TileVm, row: &Row, frame: &Rect, dy: f64, alpha: f64) {
        let awaiting = tile.awaiting();
        let (pad_v, pad_h, radius) = if awaiting { (CARD_PAD_V, CARD_PAD_H, CARD_RADIUS) } else { (TILE_PAD_V, TILE_PAD_H, TILE_RADIUS) };
        let hovered = sh.hovered.pane() == Some(tile.pane_id);
        let pressed = sh.pressed == ListHit::Tile(tile.pane_id);
        let shifted = |r: &Rect| Rect::new(r.x, r.y + dy, r.w, r.h);

        // Ground, border, focus ring.
        let fill = if awaiting {
            tokens::AWAITING_BG
        } else if pressed {
            tokens::TILE_PRESSED
        } else if hovered || tile.focused {
            tokens::TILE_HOVER
        } else if tile.minimized {
            tokens::GROUND
        } else {
            tokens::TILE
        };
        fill_round(frame, radius, fill, alpha);
        if awaiting {
            NSGraphicsContext::saveGraphicsState_class();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(frame.cg(), radius, radius).addClip();
            fill_rect(&Rect::new(frame.x, frame.y, CARD_BAR_W, frame.h), tokens::AWAITING, alpha);
            NSGraphicsContext::restoreGraphicsState_class();
            stroke_round(frame, radius, 1.0, tokens::AWAITING, if hovered { 0.8 } else { 0.35 } * alpha);
        } else if tile.minimized {
            stroke_round(frame, radius, 1.0, tokens::BORDER_SUBTLE, alpha);
        }
        if tile.focused {
            stroke_round(frame, radius, 1.5, tokens::ACCENT, alpha);
        }

        // Row 1: state glyph, title, chip or hover actions or age.
        let text_x = frame.x + TEXT_X;
        let right = frame.right() - pad_h;
        let line1 = Rect::new(text_x, frame.y + pad_v, right - text_x, LINE_TITLE_H);
        let cy = line1.y + LINE_TITLE_H / 2.0;
        if tile.state.neutral() {
            stroke_ring(frame.x + GLYPH_X + GLYPH_D / 2.0, cy, GLYPH_D, 1.5, tokens::BORDER_STRONG, alpha);
        } else {
            fill_dot(frame.x + GLYPH_X + GLYPH_D / 2.0, cy, GLYPH_D, tile.state.color(), alpha);
        }
        let mut title_right = right;
        if hovered && !row.glyphs.is_empty() {
            for (b, r) in &row.glyphs {
                let r = shifted(r);
                let b_hovered = sh.hovered == ListHit::TileButton(tile.pane_id, *b);
                let b_pressed = sh.pressed == ListHit::TileButton(tile.pane_id, *b);
                fill_round(&r, BUTTON_D / 2.0, if b_pressed { tokens::BORDER_STRONG } else { tokens::TILE_PRESSED }, alpha);
                if b_hovered && !b_pressed {
                    stroke_round(&r, BUTTON_D / 2.0, 1.0, tokens::BORDER_STRONG, alpha);
                }
                let fg = match b {
                    TileButton::Close if b_hovered => tokens::ERROR,
                    TileButton::Stop => tokens::INTERRUPT,
                    TileButton::StartClaude | TileButton::Resume => tokens::ACCENT,
                    _ if b_hovered => tokens::TEXT_PRIMARY,
                    _ => tokens::TEXT_SECONDARY,
                };
                draw_text(sh, b.glyph(), &r, Style::Glyph, fg, alpha, Align::Center, false);
                title_right = title_right.min(r.x - 6.0);
            }
        } else if let Some((age, aging)) = &tile.age {
            let w = draw_text(sh, age, &line1, Style::Secondary, if *aging { tokens::ERROR } else { tokens::TEXT_TERTIARY }, alpha, Align::Right, false);
            title_right -= w + 8.0;
        } else if !awaiting {
            let dot = matches!(tile.state, TileState::Unread { .. }).then_some(tokens::ACCENT);
            let chip = draw_chip(sh, &tile.chip(), right, cy, tile.state.chip_bg(), tile.state.chip_fg(), dot, alpha);
            title_right = chip.x - 8.0;
        }
        let title = if tile.minimized { format!("\u{229f} {}", tile.title) } else { tile.title.clone() };
        draw_text(sh, &title, &Rect::new(text_x, line1.y, (title_right - text_x).max(0.0), LINE_TITLE_H), Style::Title, tokens::TEXT_PRIMARY, alpha, Align::Left, tile.renaming);

        let mut y = line1.bottom() + LINE_GAP;
        if awaiting {
            let q_h = row.question_lines as f64 * LINE_Q_H;
            draw_wrapped(sh, tile.question.as_deref().unwrap_or(""), &Rect::new(text_x, y, right - text_x, q_h), Style::Question, tokens::TEXT_PRIMARY, alpha);
            y += q_h;
            if let Some(detail) = &tile.detail {
                y += LINE_GAP;
                draw_text(sh, detail, &Rect::new(text_x, y, right - text_x, LINE_SEC_H), Style::Detail, tokens::TEXT_SECONDARY, alpha, Align::Left, false);
            }
            for (b, r) in &row.actions {
                let r = shifted(r);
                let b_hovered = sh.hovered == ListHit::TileButton(tile.pane_id, *b);
                let b_pressed = sh.pressed == ListHit::TileButton(tile.pane_id, *b);
                match b {
                    TileButton::Open => {
                        fill_round(&r, ACTIONS_H / 2.0, if b_hovered || b_pressed { tokens::ACCENT_PRESSED } else { tokens::ACCENT }, alpha);
                        draw_text(sh, OPEN_LABEL, &r, Style::Link, tokens::TEXT_ON_FILL, alpha, Align::Center, false);
                    }
                    _ => {
                        let fg = if b_hovered { tokens::TEXT_PRIMARY } else { tokens::INTERRUPT };
                        draw_text(sh, STOP_LABEL, &r, Style::Link, fg, alpha, Align::Center, false);
                    }
                }
            }
        } else {
            let line2 = Rect::new(text_x, y, right - text_x, LINE_SEC_H);
            let mut sec_right = right;
            for (b, r) in &row.actions {
                let label = match b {
                    TileButton::StartClaude => START_CLAUDE_LABEL,
                    TileButton::Resume => RESUME_LABEL,
                    _ => continue,
                };
                let r = shifted(r);
                let b_hovered = sh.hovered == ListHit::TileButton(tile.pane_id, *b);
                draw_text(sh, label, &r, Style::Link, if b_hovered { tokens::TEXT_PRIMARY } else { tokens::ACCENT }, alpha, Align::Center, false);
                sec_right = r.x - 6.0;
            }
            let mut sx = text_x;
            if tile.bookmarked {
                let w = draw_text(sh, "\u{2605}", &Rect::new(sx, line2.y, 14.0, LINE_SEC_H), Style::Secondary, tokens::AWAITING, alpha, Align::Left, false);
                sx += w + 4.0;
            }
            draw_text(sh, &tile.secondary, &Rect::new(sx, line2.y, (sec_right - sx).max(0.0), LINE_SEC_H), Style::Secondary, tokens::TEXT_SECONDARY, alpha, Align::Left, false);
            if let Some(summary) = &tile.summary {
                let line3_y = line2.bottom() + LINE_GAP;
                draw_text(sh, summary, &Rect::new(text_x, line3_y, right - text_x, LINE_SEC_H), Style::Secondary, tokens::TEXT_SECONDARY, alpha, Align::Left, false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::sidebar::{CollapsedSummary, PaneFlags};

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

    fn tile(pane_id: PaneId, column: usize, state: TileState) -> TileVm {
        TileVm {
            pane_id,
            column,
            state,
            minimized: false,
            bare_shell: state == TileState::Shell,
            resumable: false,
            title: format!("pane {pane_id}"),
            secondary: "link \u{b7} claude".into(),
            agent: Some("claude".into()),
            bookmarked: false,
            focused: false,
            renaming: false,
            question: None,
            detail: None,
            summary: None,
            age: None,
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

    /// Tab 0 expanded with two tiles, tab 1 collapsed, tab 2 with one tile.
    fn model() -> SidebarModel {
        SidebarModel {
            summary: Vec::new(),
            sort: SidebarSort::Kova,
            pill: NextPill::Nothing,
            groups: vec![
                group(0, false, vec![tile(10, 0, TileState::Idle), tile(11, 0, TileState::Working)]),
                group(1, true, vec![tile(20, 0, TileState::Idle)]),
                group(2, false, vec![tile(30, 0, TileState::Shell)]),
            ],
            show_hint: false,
        }
    }

    const W: f64 = 280.0;
    /// Content x: 12 + 3 + 12.
    const CX: f64 = 27.0;

    #[test]
    fn rows_stack_with_gaps_and_the_tint_bar_spans_each_group() {
        let l = ListLayout::new(&model(), W, &FakeMetrics);
        let ys: Vec<f64> = l.rows.iter().map(|r| r.frame.y).collect();
        // header 6..34, gap 6, tile 40..94, gap 6, tile 100..154, group gap
        // 16, header 170..198 (collapsed), gap 16, header 214..242, gap 6,
        // tile 248..302, bottom 12.
        assert_eq!(ys, vec![6.0, 40.0, 100.0, 170.0, 214.0, 248.0]);
        assert_eq!(l.rows[1].frame.h, 54.0);
        assert_eq!(l.rows[1].frame.x, CX);
        assert_eq!(l.rows[1].frame.right(), W - 8.0);
        assert_eq!(l.groups[0], Rect::new(12.0, 6.0, W - 20.0, 148.0));
        assert_eq!(l.groups[1].h, HEADER_H);
        assert_eq!(l.content_h, 302.0 + 12.0);
        assert_eq!(l.hint_y, None);
    }

    #[test]
    fn tile_heights_follow_their_content() {
        let mut m = model();
        let mut awaiting = tile(1, 0, TileState::Awaiting);
        awaiting.question = Some("Should I overwrite hello.txt with the new content?".into());
        awaiting.detail = Some("Write hello.txt".into());
        awaiting.age = Some(("4m".into(), false));
        let mut short = tile(2, 0, TileState::Awaiting);
        short.question = Some("Run it?".into());
        let mut unread = tile(3, 0, TileState::Unread { bell: false });
        unread.summary = Some("Pushed the copy".into());
        m.groups = vec![group(0, false, vec![awaiting, short, unread, tile(4, 0, TileState::Working)])];
        let l = ListLayout::new(&m, W, &FakeMetrics);
        let hs: Vec<f64> = l.rows.iter().map(|r| r.frame.h).collect();
        // 50 chars * 6 = 300 > 217 wide: two lines. 10 + 20 + 2 + 32 + 2 +
        // 16 + 4 + 20 + 10 = 116; one line, no detail: 10 + 20 + 2 + 16 + 4
        // + 20 + 10 = 82; unread with a summary: 72; plain: 54.
        assert_eq!(hs, vec![HEADER_H, 116.0, 82.0, 72.0, 54.0]);
        assert_eq!(l.rows[1].question_lines, 2);
        assert_eq!(l.rows[2].question_lines, 1);
        // The card's actions: Open at the text column, Stop at the right.
        let open = l.rows[1].actions.iter().find(|(b, _)| *b == TileButton::Open).unwrap().1;
        assert_eq!(open.x, CX + TEXT_X);
        assert_eq!(open.w, 4.0 * 6.0 + 20.0);
        let stop = l.rows[1].actions.iter().find(|(b, _)| *b == TileButton::Stop).unwrap().1;
        assert_eq!(stop.right(), W - 8.0 - CARD_PAD_H);
        assert_eq!(open.y, stop.y);
    }

    #[test]
    fn the_hint_sits_under_the_last_group() {
        let mut m = model();
        m.show_hint = true;
        let l = ListLayout::new(&m, W, &FakeMetrics);
        assert_eq!(l.hint_y, Some(302.0 + 16.0));
        assert_eq!(l.content_h, 302.0 + 16.0 + 20.0 + 12.0);
        // An empty window still lays out.
        m.groups.clear();
        let l = ListLayout::new(&m, W, &FakeMetrics);
        assert_eq!(l.hint_y, Some(6.0));
        assert!(l.rows.is_empty());
    }

    #[test]
    fn hit_tells_the_rows_and_their_buttons_apart() {
        let l = ListLayout::new(&model(), W, &FakeMetrics);
        // Header: chevron zone, body, `+`.
        assert_eq!(l.hit(CX + 5.0, 20.0), ListHit::Chevron(0));
        assert_eq!(l.hit(CX + 60.0, 20.0), ListHit::Header(0));
        let add = ListLayout::add_button(&l.rows[0].frame);
        assert_eq!(l.hit(add.x + 3.0, add.y + 3.0), ListHit::HeaderAdd(0));
        // The gap above a tile is nothing, the body is the pane.
        assert_eq!(l.hit(100.0, 36.0), ListHit::Empty);
        assert_eq!(l.hit(100.0, 60.0), ListHit::Tile(10));
        assert_eq!(l.hit(100.0, 120.0), ListHit::Tile(11));
        // Left of the content column: nothing.
        assert_eq!(l.hit(5.0, 60.0), ListHit::Empty);
        assert_eq!(l.hit(100.0, 180.0), ListHit::Header(1));
        assert_eq!(l.hit(100.0, 260.0), ListHit::Tile(30));
        assert_eq!(l.hit(100.0, 1000.0), ListHit::Empty);
        // Working tile: close, minimize, stop boxes from the right on line 1.
        let row = &l.rows[2];
        let boxes: Vec<TileButton> = row.glyphs.iter().map(|(b, _)| *b).collect();
        assert_eq!(boxes, vec![TileButton::Close, TileButton::Minimize, TileButton::Stop]);
        let (_, close) = row.glyphs[0];
        assert_eq!(close.right(), W - 8.0 - TILE_PAD_H);
        assert_eq!(l.hit(close.x + 5.0, close.y + 5.0), ListHit::TileButton(11, TileButton::Close));
        let (_, stop) = row.glyphs[2];
        // Minimize sits one gap left of close, stop one gap left of minimize.
        assert_eq!(stop.right(), close.x - 2.0 * BUTTON_GAP - BUTTON_D);
        assert_eq!(l.hit(stop.x + 5.0, stop.y + 5.0), ListHit::TileButton(11, TileButton::Stop));
        // Line 2 carries no glyphs.
        assert_eq!(l.hit(close.x + 5.0, close.y + 30.0), ListHit::Tile(11));
        // The bare shell's `▶ Start Claude` sits on line 2.
        let shell = &l.rows[5];
        let (b, start) = shell.actions[0];
        assert_eq!(b, TileButton::StartClaude);
        assert_eq!(l.hit(start.x + 5.0, start.y + 5.0), ListHit::TileButton(30, TileButton::StartClaude));
        assert_eq!(ListHit::TileButton(30, TileButton::Open).pane(), Some(30));
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
        assert_eq!(resume.w, RESUME_LABEL.chars().count() as f64 * 6.0 + 8.0);
        assert_eq!(resume.right(), W - 8.0 - TILE_PAD_H);
        assert_eq!(l.hit(resume.x + 5.0, resume.y + 5.0), ListHit::TileButton(7, TileButton::Resume));
        assert_eq!(l.rows[1].glyphs.iter().map(|(b, _)| *b).collect::<Vec<_>>(), vec![TileButton::Close, TileButton::Minimize, TileButton::Resume]);
        assert_eq!(l.rows[2].actions[0].0, TileButton::StartClaude);
        assert_eq!(l.rows[1].frame.h, l.rows[2].frame.h);
    }

    #[test]
    fn insertion_index_follows_the_midpoint_rule() {
        let l = ListLayout::new(&model(), W, &FakeMetrics);
        // Headers at 6..34, 170..198, 214..242.
        assert_eq!(l.insertion_index(10.0), 0);
        assert_eq!(l.insertion_index(100.0), 1);
        assert_eq!(l.insertion_index(180.0), 1);
        assert_eq!(l.insertion_index(190.0), 2);
        assert_eq!(l.insertion_index(235.0), 3);
        assert_eq!(l.insertion_line_y(0), 6.0 - 8.0);
        assert_eq!(l.insertion_line_y(1), 170.0 - 8.0);
        assert_eq!(l.insertion_line_y(3), 302.0 + 8.0);
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
        // mid-drag) has an empty run, and an empty run has no slot.
        assert_eq!(l.pane_run(99), 99..99);
        assert_eq!(l.pane_insertion_slot(&(0..0), 50.0), None);
        assert_eq!(l.pane_insertion_slot(&(99..99), 50.0), None);
        let run = 1..3;
        // Tile 1 spans 40..94 (centre 67), tile 2 spans 100..154 (centre 127).
        assert_eq!(l.pane_insertion_slot(&run, 50.0), Some(0));
        assert_eq!(l.pane_insertion_slot(&run, 100.0), Some(1));
        assert_eq!(l.pane_insertion_slot(&run, 140.0), Some(2));
        // Far above or below the run: no slot, the drop snaps back.
        assert_eq!(l.pane_insertion_slot(&run, 5.0), None);
        assert_eq!(l.pane_insertion_slot(&run, 300.0), None);
        assert_eq!(l.pane_insertion_line_y(&run, 0), 40.0 - 3.0);
        assert_eq!(l.pane_insertion_line_y(&run, 1), 100.0 - 3.0);
        assert_eq!(l.pane_insertion_line_y(&run, 2), 154.0 + 3.0);
        assert_eq!(l.row_for_pane(3), Some(3));
        assert_eq!(l.row_for_pane(99), None);
        assert_eq!(l.row_for_group(1), Some(4));
    }

    #[test]
    fn the_chrome_stacks_top_area_summary_pill_list_and_footer() {
        let c = ChromeLayout::new(W, 600.0, 40.0, 50.0);
        assert_eq!(c.summary, Rect::new(12.0, 36.0, W - 1.0 - 24.0, 24.0));
        assert_eq!(c.sort.right(), c.summary.right());
        assert_eq!(c.sort.w, 52.0);
        assert_eq!(c.pill, Rect::new(12.0, 64.0, W - 1.0 - 24.0, 28.0));
        assert_eq!(c.list, Rect::new(0.0, 96.0, W - 1.0 - 4.0, 600.0 - 96.0 - 24.0));
        assert_eq!(c.footer.y, 576.0);
        assert_eq!(c.mode_button.right(), W - 1.0 - 12.0);
        assert_eq!(c.hit(10.0, 10.0), ChromeHit::TopArea);
        assert_eq!(c.hit(10.0, 45.0), ChromeHit::Empty);
        assert_eq!(c.hit(c.sort.x + 5.0, 45.0), ChromeHit::SortToggle);
        assert_eq!(c.hit(100.0, 70.0), ChromeHit::NextPill);
        assert_eq!(c.hit(100.0, 61.0), ChromeHit::Empty);
        assert_eq!(c.hit(100.0, 300.0), ChromeHit::Empty);
        assert_eq!(c.hit(c.mode_button.x + 5.0, 585.0), ChromeHit::ModeButton);
        assert_eq!(c.hit(10.0, 585.0), ChromeHit::Empty);
        // The resize handle wins near the separator, on both sides.
        assert_eq!(c.hit(W - 4.0, 300.0), ChromeHit::Edge);
        assert_eq!(c.hit(W + 3.0, 300.0), ChromeHit::Edge);
        assert_eq!(c.hit(W - 6.0, 300.0), ChromeHit::Empty);
        assert_eq!(c.hit(W + 6.0, 300.0), ChromeHit::Empty);
    }

    #[test]
    fn styles_match_the_phone_rows() {
        // KovaLink's `calloutStrong`, `footnote` and `caption`.
        assert_eq!(Style::Title.size(), 15.0);
        assert_eq!(Style::Title.weight(), Weight::Semibold);
        assert_eq!(Style::Secondary.size(), 13.0);
        assert_eq!(Style::Secondary.weight(), Weight::Regular);
        assert_eq!(Style::Chip.size(), 11.0);
        assert_eq!(Style::Chip.weight(), Weight::Medium);
        assert_eq!(Style::PillBadge.weight(), Weight::Bold);
        assert_eq!(Style::ALL.len(), 15);
        let _ = PaneFlags::default();
    }
}
