//! The sidebar layout mode: the tabs of a window as collapsible groups down
//! the left edge, one tile per pane, instead of the strip across the top.
//! The look and the vocabulary are KovaLink's home screen: coloured tab
//! bands, tiles with a state bar and a chip, the summary line and the Next
//! pill, hover glyph actions.
//!
//! This module holds everything about it that can be checked without a
//! window: the process-wide layout setting and its persistence, the tile
//! layout and hit-testing (`SidebarGeometry`), the state vocabulary
//! (`TileState`), the activity sort, the pane drag arithmetic, the colour
//! tokens and the text rules. The renderer paints from the same geometry the
//! mouse handlers consult, so a click lands on the tile that was drawn. See
//! `docs/sidebar-spec.md`.

use std::sync::atomic::{AtomicU16, AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

use crate::config::{clamp_sidebar_width, LayoutConfig, LayoutMode, LayoutPrefs};
use crate::pane::PaneId;

// ---------------------------------------------------------------
// Process-wide layout setting
// ---------------------------------------------------------------
//
// The mode and the sidebar width are one setting for the whole app, not one
// per window: the View menu, the shortcut and the edge drag all change "how
// Kova shows tabs". Every window reads them on each frame.

static MODE: AtomicU8 = AtomicU8::new(0);
static WIDTH_CELLS: AtomicU16 = AtomicU16::new(32);

/// Adopt the layout the config resolved to (`[layout]` table plus the
/// `prefs.json` overrides). Called once at startup.
pub fn init(layout: &LayoutConfig) {
    MODE.store(mode_to_u8(layout.mode), Ordering::Relaxed);
    WIDTH_CELLS.store(clamp_sidebar_width(layout.sidebar_width), Ordering::Relaxed);
}

fn mode_to_u8(mode: LayoutMode) -> u8 {
    match mode {
        LayoutMode::Tabs => 0,
        LayoutMode::Sidebar => 1,
    }
}

pub fn layout_mode() -> LayoutMode {
    if MODE.load(Ordering::Relaxed) == 1 { LayoutMode::Sidebar } else { LayoutMode::Tabs }
}

/// Change the mode and remember it for the next launch.
pub fn set_layout_mode(mode: LayoutMode) {
    MODE.store(mode_to_u8(mode), Ordering::Relaxed);
    persist();
}

pub fn width_cells() -> u16 {
    WIDTH_CELLS.load(Ordering::Relaxed)
}

/// Set the width (snapped into range). Returns whether it changed. Not
/// persisted here: an edge drag calls this on every event and `persist`
/// once, on mouse up.
pub fn set_width_cells(cells: u16) -> bool {
    let cells = clamp_sidebar_width(cells);
    WIDTH_CELLS.swap(cells, Ordering::Relaxed) != cells
}

/// Write the runtime layout state to `prefs.json`.
pub fn persist() {
    LayoutPrefs { mode: Some(layout_mode()), sidebar_width: Some(width_cells()) }.save();
}

// ---------------------------------------------------------------
// Colour tokens (KovaLink `theme/tokens.ts`, dark set)
// ---------------------------------------------------------------

/// The palette the sidebar is painted with, shared with the phone app so the
/// two screens read the same. RGB floats.
pub mod tokens {
    pub const GROUND: [f32; 3] = [0.043, 0.051, 0.063];
    pub const TILE: [f32; 3] = [0.078, 0.090, 0.110];
    pub const TILE_HOVER: [f32; 3] = [0.106, 0.122, 0.149];
    pub const TILE_PRESSED: [f32; 3] = [0.122, 0.141, 0.169];
    pub const BORDER_SUBTLE: [f32; 3] = [0.137, 0.153, 0.184];
    pub const BORDER_STRONG: [f32; 3] = [0.200, 0.224, 0.267];
    pub const ACCENT: [f32; 3] = [0.298, 0.553, 1.000];
    pub const ACCENT_PRESSED: [f32; 3] = [0.227, 0.475, 0.902];
    pub const TEXT_PRIMARY: [f32; 3] = [0.910, 0.918, 0.929];
    pub const TEXT_SECONDARY: [f32; 3] = [0.608, 0.639, 0.686];
    pub const TEXT_TERTIARY: [f32; 3] = [0.486, 0.522, 0.576];
    pub const TEXT_INVERSE: [f32; 3] = [0.043, 0.051, 0.063];
    pub const TEXT_ON_FILL: [f32; 3] = [1.0, 1.0, 1.0];
    pub const AWAITING: [f32; 3] = [1.000, 0.690, 0.125];
    pub const AWAITING_BG: [f32; 3] = [0.165, 0.122, 0.031];
    pub const WORKING: [f32; 3] = [0.220, 0.741, 0.973];
    pub const SUCCESS: [f32; 3] = [0.239, 0.839, 0.549];
    pub const ERROR: [f32; 3] = [1.000, 0.361, 0.361];
    pub const INTERRUPT: [f32; 3] = [1.000, 0.478, 0.478];
    pub const SEPARATOR: [f32; 3] = BORDER_SUBTLE;
}

/// Relative luminance of a colour, for the text on a coloured band.
pub fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Text colour on the active tab's band: dark on a light band (yellow,
/// green), white on the others.
pub fn on_band(c: [f32; 3]) -> [f32; 3] {
    if luminance(c) > 0.55 { tokens::TEXT_INVERSE } else { tokens::TEXT_ON_FILL }
}

/// The tint an inactive tab's band takes: its (already dimmed) colour at 22%
/// over the ground, KovaLink's `${tint}22`.
pub fn band_tint(dimmed: [f32; 3]) -> [f32; 3] {
    let g = tokens::GROUND;
    [
        g[0] + (dimmed[0] - g[0]) * 0.22,
        g[1] + (dimmed[1] - g[1]) * 0.22,
        g[2] + (dimmed[2] - g[2]) * 0.22,
    ]
}

// ---------------------------------------------------------------
// Sort order
// ---------------------------------------------------------------

/// How the sidebar orders a window's tabs: the tab order (what Cmd+1..9
/// count), or the tabs asking for something first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarSort {
    #[default]
    Kova,
    Activity,
}

impl SidebarSort {
    pub fn toggled(self) -> Self {
        match self {
            SidebarSort::Kova => SidebarSort::Activity,
            SidebarSort::Activity => SidebarSort::Kova,
        }
    }

    /// Copy of the sort toggle in the summary row.
    pub fn label(self) -> &'static str {
        match self {
            SidebarSort::Kova => "\u{21c5} kova",
            SidebarSort::Activity => "\u{21c5} activity",
        }
    }
}

/// The order tabs are listed in: their own order, or by the most urgent state
/// of any of their panes (`tab_states[i]` is that state for tab `i`), ties
/// keeping the tab order so nothing jumps around while states are equal.
pub fn display_order(sort: SidebarSort, tab_states: &[TileState]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..tab_states.len()).collect();
    if sort == SidebarSort::Activity {
        order.sort_by_key(|&i| tab_states[i].rank());
    }
    order
}

// ---------------------------------------------------------------
// Tile states
// ---------------------------------------------------------------

/// What a pane tile says, in priority order: the first variant that applies
/// wins, and the order doubles as urgency for the activity sort and the
/// collapsed chips. Same vocabulary as KovaLink's badges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileState {
    /// A permission prompt is on screen (detected, never the hook flag alone).
    Awaiting,
    /// Output nobody looked at: a finished turn, a completion, or a bell.
    Unread { bell: bool },
    Working,
    /// Claude launched, its session not resolved yet.
    Starting,
    /// An agent session sitting open and quiet.
    Idle,
    /// A plain shell.
    Shell,
}

/// The `Pane` reads a tile state is derived from. `seen` is true for the
/// focused pane of the active tab: what is being looked at is never unread.
#[derive(Clone, Copy, Debug, Default)]
pub struct PaneFlags {
    pub permission_prompt: bool,
    pub bell: bool,
    pub completion: bool,
    /// The hook's waiting flag, unseen. Paints `done`, not `waiting`: the
    /// `Stop` hook raises it at every turn end.
    pub hook_unseen: bool,
    pub turn_end_unseen: bool,
    pub working: bool,
    pub starting: bool,
    pub idle_agent: bool,
    pub seen: bool,
}

impl TileState {
    pub fn from_flags(f: PaneFlags) -> Self {
        if f.permission_prompt && !f.working {
            TileState::Awaiting
        } else if !f.seen && (f.completion || f.bell || f.hook_unseen || f.turn_end_unseen) {
            TileState::Unread { bell: f.bell && !f.completion }
        } else if f.working {
            TileState::Working
        } else if f.starting {
            TileState::Starting
        } else if f.idle_agent {
            TileState::Idle
        } else {
            TileState::Shell
        }
    }

    /// Urgency, lowest first.
    pub fn rank(self) -> u8 {
        match self {
            TileState::Awaiting => 0,
            TileState::Unread { .. } => 1,
            TileState::Working => 2,
            TileState::Starting => 3,
            TileState::Idle => 4,
            TileState::Shell => 5,
        }
    }

    /// The most urgent of a group's states, `Shell` for an empty group.
    pub fn most_urgent(states: impl Iterator<Item = TileState>) -> TileState {
        states.min_by_key(|s| s.rank()).unwrap_or(TileState::Shell)
    }

    /// Chip copy. The awaiting tile shows its age instead of a chip.
    pub fn chip(self) -> &'static str {
        match self {
            TileState::Awaiting => "waiting",
            TileState::Unread { bell: true } => "\u{25cf} bell",
            TileState::Unread { bell: false } => "\u{25cf} done",
            TileState::Working => "working",
            TileState::Starting => "starting",
            TileState::Idle => "idle",
            TileState::Shell => "shell",
        }
    }

    /// Colour of the state bar and the chip fill. Neutral states take
    /// `border.strong`, so every tile has a bar.
    pub fn color(self) -> [f32; 3] {
        match self {
            TileState::Awaiting => tokens::AWAITING,
            TileState::Unread { .. } => tokens::ACCENT,
            TileState::Working | TileState::Starting => tokens::WORKING,
            TileState::Idle | TileState::Shell => tokens::BORDER_STRONG,
        }
    }

    /// Neutral chips (idle, shell) are grey on `bg.overlay`, not inverted.
    pub fn neutral(self) -> bool {
        matches!(self, TileState::Idle | TileState::Shell)
    }
}

/// What a collapsed header says about its panes: chips for the awaiting and
/// working counts, then the pane count (`collapsedSummary()` on the phone).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CollapsedSummary {
    pub awaiting: usize,
    pub working: usize,
    pub count: usize,
}

impl CollapsedSummary {
    pub fn of(states: impl Iterator<Item = TileState>) -> Self {
        let mut s = CollapsedSummary::default();
        for st in states {
            s.count += 1;
            match st {
                TileState::Awaiting => s.awaiting += 1,
                TileState::Working => s.working += 1,
                _ => {}
            }
        }
        s
    }

    /// `k panes` / `1 pane`.
    pub fn count_label(&self) -> String {
        if self.count == 1 { "1 pane".to_string() } else { format!("{} panes", self.count) }
    }

    /// Cells the summary takes: each chip is `(digits + 2)` cells plus a
    /// one-cell gap, then the count label.
    pub fn cells(&self) -> usize {
        let chip = |n: usize| if n > 0 { n.to_string().len() + 3 } else { 0 };
        chip(self.awaiting) + chip(self.working) + self.count_label().chars().count()
    }
}

// ---------------------------------------------------------------
// Tile actions
// ---------------------------------------------------------------

/// The click targets a tile carries besides its body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileButton {
    Close,
    Minimize,
    Restore,
    /// Interrupt (`■`): the hover glyph, and the awaiting tile's `■ Stop`.
    Stop,
    /// `▶`: the hover glyph, and the bare shell tile's `▶ Start Claude`.
    StartClaude,
    /// The awaiting tile's `Open ⏎`.
    Open,
}

impl TileButton {
    pub fn glyph(self) -> &'static str {
        match self {
            TileButton::Close => "\u{d7}",
            TileButton::Minimize => "\u{229f}",
            TileButton::Restore => "\u{229e}",
            TileButton::Stop => "\u{25a0}",
            TileButton::StartClaude => "\u{25b6}",
            TileButton::Open => "\u{23ce}",
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            TileButton::Close => "Close",
            TileButton::Minimize => "Minimize",
            TileButton::Restore => "Restore",
            TileButton::Stop => "Stop",
            TileButton::StartClaude => "Start Claude here",
            TileButton::Open => "Open",
        }
    }
}

/// The shape of one tile: enough to lay it out and to know which buttons it
/// carries, without the pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileLayout {
    pub state: TileState,
    pub minimized: bool,
    /// No agent, no foreground process, nothing pending: `▶ Start Claude`.
    pub bare_shell: bool,
    /// Text lines; the tile is one more `ch` tall.
    pub lines: u8,
}

impl TileLayout {
    /// The hover glyphs on line 0, right to left: close, minimize / restore,
    /// stop when something can be interrupted, start Claude on a bare shell.
    pub fn glyphs(&self) -> Vec<TileButton> {
        let mut out = vec![
            TileButton::Close,
            if self.minimized { TileButton::Restore } else { TileButton::Minimize },
        ];
        if matches!(self.state, TileState::Working | TileState::Awaiting) {
            out.push(TileButton::Stop);
        }
        if self.bare_shell {
            out.push(TileButton::StartClaude);
        }
        out
    }

    pub fn awaiting(&self) -> bool {
        self.state == TileState::Awaiting
    }
}

/// Copy of the awaiting tile's action line and the shell tile's call.
pub const OPEN_LABEL: &str = "Open \u{23ce}";
pub const STOP_LABEL: &str = "\u{25a0} Stop";
pub const START_CLAUDE_LABEL: &str = "\u{25b6} Start Claude";

// ---------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarRowKind {
    Header { tab_idx: usize, collapsed: bool },
    Pane { tab_idx: usize, pane_id: PaneId, column: usize, tile: TileLayout },
}

/// One drawn row: its kind, and its vertical extent in list content space
/// (0 = top of the list before any scroll).
#[derive(Clone, Copy, Debug)]
pub struct SidebarRow {
    pub kind: SidebarRowKind,
    pub y: f32,
    pub h: f32,
}

/// What a point of the sidebar lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarHit {
    /// The chevron cells of a group header: fold or unfold, no tab switch.
    Chevron(usize),
    /// The rest of a group header: switch to the tab (or fold the active one).
    Header(usize),
    /// The `+` at the right of a header: add a pane to that tab.
    HeaderAdd(usize),
    Pane(PaneId),
    /// A glyph or action line of a tile.
    PaneButton(PaneId, TileButton),
    SortToggle,
    NextPill,
    ModeButton,
    /// The resize handle at the right edge.
    Edge,
    /// The traffic-light strip: window drag region.
    TopArea,
    Empty,
}

impl SidebarHit {
    /// The pane a hit belongs to, for hover on any part of a tile.
    pub fn pane(self) -> Option<PaneId> {
        match self {
            SidebarHit::Pane(id) | SidebarHit::PaneButton(id, _) => Some(id),
            _ => None,
        }
    }

    /// The tab a header hit belongs to.
    pub fn header(self) -> Option<usize> {
        match self {
            SidebarHit::Chevron(t) | SidebarHit::Header(t) | SidebarHit::HeaderAdd(t) => Some(t),
            _ => None,
        }
    }
}

/// Every pixel position the renderer and the mouse handlers agree on. All
/// heights are rounded to whole pixels so glyphs sit on the atlas grid.
#[derive(Clone, Debug)]
pub struct SidebarGeometry {
    pub cell_w: f32,
    pub cell_h: f32,
    pub scale: f32,
    pub width_cells: u16,
    /// Sidebar width in pixels, without the separator.
    pub width: f32,
    /// Separator line right of the sidebar.
    pub sep_w: f32,
    /// Sidebar height: the window minus the global status bar.
    pub height: f32,
    pub top_h: f32,
    pub summary_h: f32,
    /// Next pill: top and height (the region around it is `pill_region_h`).
    pub pill_y: f32,
    pub pill_h: f32,
    pub header_h: f32,
    pub footer_h: f32,
    pub list_y: f32,
    pub list_h: f32,
    pub scroll_y: f32,
    pub rows: Vec<SidebarRow>,
    /// Height of everything in the list, gaps and hint included.
    pub content_h: f32,
    /// Content-space top of the "new tab / split" hint, when it is shown.
    pub hint_y: Option<f32>,
}

/// Cells from the left edge where a header's title starts.
pub const HEADER_TITLE_COL: f32 = 5.0;
/// Cells at the right of a collapsed header kept for its chips.
const COLLAPSED_SUMMARY_CELLS: u16 = 12;
/// Cells at the right of a header taken by the `+` and its margin.
const HEADER_ADD_CELLS: f32 = 4.0;
/// Cells of a header, from the left, that answer to the chevron.
const CHEVRON_CELLS: f32 = 3.0;
/// Cells at the right of the summary row that answer to the sort toggle.
const SORT_TOGGLE_CELLS: f32 = 12.0;
/// Cells at the right of the footer that answer to the mode button.
const MODE_BUTTON_CELLS: f32 = 11.0;
/// Points either side of the separator that grab the resize handle.
const EDGE_TOLERANCE_PT: f32 = 4.0;
/// Width of one hover glyph box, in cells.
pub const GLYPH_BOX_CELLS: f32 = 3.0;

impl SidebarGeometry {
    pub fn new(
        cell: (f32, f32),
        scale: f32,
        width_cells: u16,
        height: f32,
        scroll_y: f32,
        kinds: &[SidebarRowKind],
        show_hint: bool,
    ) -> Self {
        let (cell_w, cell_h) = cell;
        let width_cells = clamp_sidebar_width(width_cells);
        let top_h = (cell_h * 2.0).round();
        let summary_h = (cell_h * 1.5).round();
        let pill_margin = (cell_h * 0.25).round();
        let pill_h = (cell_h * 1.5).round();
        let pill_y = top_h + summary_h + pill_margin;
        let header_h = (cell_h * 2.0).round();
        let tile_gap = (cell_h * 0.5).round();
        let group_gap = cell_h.round();
        let footer_h = (cell_h * 1.5).round();
        let list_y = top_h + summary_h + (cell_h * 2.0).round();
        let list_h = (height - list_y - footer_h).max(0.0);

        let mut rows = Vec::with_capacity(kinds.len());
        let mut y = 0.0_f32;
        let mut prev_was_pane = false;
        for &kind in kinds {
            let h = match kind {
                SidebarRowKind::Header { .. } => {
                    if prev_was_pane {
                        y += group_gap;
                    }
                    header_h
                }
                SidebarRowKind::Pane { tile, .. } => {
                    y += tile_gap;
                    (cell_h * (tile.lines as f32 + 1.0)).round()
                }
            };
            rows.push(SidebarRow { kind, y, h });
            y += h;
            prev_was_pane = matches!(kind, SidebarRowKind::Pane { .. });
        }
        if prev_was_pane {
            y += group_gap;
        }
        let hint_y = if show_hint {
            let hy = y;
            y += header_h;
            Some(hy)
        } else {
            None
        };
        let content_h = y;

        let mut g = SidebarGeometry {
            cell_w,
            cell_h,
            scale,
            width_cells,
            width: (width_cells as f32 * cell_w).round(),
            sep_w: scale.round().max(1.0),
            height,
            top_h,
            summary_h,
            pill_y,
            pill_h,
            header_h,
            footer_h,
            list_y,
            list_h,
            scroll_y: 0.0,
            rows,
            content_h,
            hint_y,
        };
        g.scroll_y = g.clamp_scroll(scroll_y);
        g
    }

    /// Sidebar plus separator: where the panes start.
    pub fn total_w(&self) -> f32 {
        self.width + self.sep_w
    }

    pub fn max_scroll(&self) -> f32 {
        (self.content_h - self.list_h).max(0.0)
    }

    pub fn clamp_scroll(&self, scroll_y: f32) -> f32 {
        scroll_y.clamp(0.0, self.max_scroll())
    }

    /// Whether rows are hidden above / below the visible part of the list.
    pub fn overflow(&self) -> (bool, bool) {
        (self.scroll_y > 0.5, self.content_h - self.scroll_y > self.list_h + 0.5)
    }

    /// Screen y of a row's top, scroll applied.
    pub fn row_screen_y(&self, idx: usize) -> f32 {
        self.list_y + self.rows[idx].y - self.scroll_y
    }

    /// Screen y of the hint line, if shown.
    pub fn hint_screen_y(&self) -> Option<f32> {
        self.hint_y.map(|y| self.list_y + y - self.scroll_y)
    }

    /// The Next pill's horizontal extent: one cell in from each edge.
    pub fn pill_x(&self) -> (f32, f32) {
        (self.cell_w, self.width - self.cell_w)
    }

    // --- tiles ---

    /// Left edge of every tile.
    pub fn tile_x(&self) -> f32 {
        self.cell_w
    }

    pub fn tile_w(&self) -> f32 {
        self.width - 2.0 * self.cell_w
    }

    /// Width of the state bar covering a tile's left border.
    pub fn bar_w(&self) -> f32 {
        (self.cell_w * 0.5).round()
    }

    /// Where a tile's text starts.
    pub fn content_x(&self) -> f32 {
        self.tile_x() + (self.cell_w * 1.5).round()
    }

    /// Where a tile's text ends (right-aligned chips and glyphs end here).
    pub fn content_right(&self) -> f32 {
        self.tile_x() + self.tile_w() - self.cell_w
    }

    /// Cells available to a tile's text from `content_x` to `content_right`.
    pub fn content_cells(&self) -> usize {
        ((self.content_right() - self.content_x()) / self.cell_w).floor().max(0.0) as usize
    }

    /// Screen y of text line `k` of a tile whose top is at `tile_y`.
    pub fn line_y(&self, tile_y: f32, k: u8) -> f32 {
        tile_y + (self.cell_h * (0.5 + k as f32)).round()
    }

    /// The hover glyph boxes of a tile, right to left on line 0: each
    /// `(button, x)` box is `GLYPH_BOX_CELLS` wide and one cell tall.
    pub fn glyph_boxes(&self, tile: &TileLayout) -> Vec<(TileButton, f32)> {
        let box_w = GLYPH_BOX_CELLS * self.cell_w;
        tile.glyphs()
            .into_iter()
            .enumerate()
            .map(|(k, b)| (b, self.content_right() - box_w * (k as f32 + 1.0)))
            .collect()
    }

    /// Cells a title may take on line 0 next to a chip of `chip_cells`.
    pub fn title_cells_beside(&self, chip_cells: usize) -> usize {
        self.content_cells().saturating_sub(chip_cells + 1)
    }

    // --- headers ---

    /// Cells available to a header title from `HEADER_TITLE_COL`: the chips
    /// need the room on a collapsed header, the `+` on every header.
    pub fn title_cells(&self, collapsed_header: bool) -> usize {
        let reserved = HEADER_TITLE_COL as u16
            + if collapsed_header { COLLAPSED_SUMMARY_CELLS } else { HEADER_ADD_CELLS as u16 };
        self.width_cells.saturating_sub(reserved) as usize
    }

    /// Screen x of the `+` cell on a header.
    pub fn header_add_x(&self) -> f32 {
        self.width - 3.0 * self.cell_w
    }

    /// Right edge of a collapsed header's chips.
    pub fn header_summary_right(&self) -> f32 {
        self.width - HEADER_ADD_CELLS * self.cell_w
    }

    /// The row a pane is listed in, if its group is expanded.
    pub fn row_for_pane(&self, pane_id: PaneId) -> Option<usize> {
        self.rows.iter().position(|r| matches!(r.kind, SidebarRowKind::Pane { pane_id: p, .. } if p == pane_id))
    }

    pub fn row_for_header(&self, tab_idx: usize) -> Option<usize> {
        self.rows.iter().position(|r| matches!(r.kind, SidebarRowKind::Header { tab_idx: t, .. } if t == tab_idx))
    }

    /// The smallest scroll change that brings a row fully into the list.
    pub fn reveal(&self, idx: usize) -> f32 {
        let row = &self.rows[idx];
        let top = row.y;
        let bottom = row.y + row.h;
        let scroll = if top < self.scroll_y {
            top
        } else if bottom > self.scroll_y + self.list_h {
            bottom - self.list_h
        } else {
            self.scroll_y
        };
        self.clamp_scroll(scroll)
    }

    /// Whether a point is inside the sidebar or on its resize handle.
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px < self.total_w() + EDGE_TOLERANCE_PT * self.scale && py >= 0.0 && py < self.height
    }

    pub fn hit(&self, px: f32, py: f32) -> Option<SidebarHit> {
        if !self.contains(px, py) {
            return None;
        }
        let edge_x = self.width + self.sep_w / 2.0;
        if (px - edge_x).abs() <= EDGE_TOLERANCE_PT * self.scale {
            return Some(SidebarHit::Edge);
        }
        if px >= self.width {
            return None;
        }
        if py < self.top_h {
            return Some(SidebarHit::TopArea);
        }
        if py < self.top_h + self.summary_h {
            return Some(if px >= self.width - SORT_TOGGLE_CELLS * self.cell_w {
                SidebarHit::SortToggle
            } else {
                SidebarHit::Empty
            });
        }
        if py < self.list_y {
            let (x0, x1) = self.pill_x();
            let inside = py >= self.pill_y && py < self.pill_y + self.pill_h && px >= x0 && px < x1;
            return Some(if inside { SidebarHit::NextPill } else { SidebarHit::Empty });
        }
        if py >= self.height - self.footer_h {
            return Some(if px >= self.width - MODE_BUTTON_CELLS * self.cell_w {
                SidebarHit::ModeButton
            } else {
                SidebarHit::Empty
            });
        }
        if py >= self.list_y + self.list_h {
            return Some(SidebarHit::Empty);
        }
        let ly = py - self.list_y + self.scroll_y;
        for row in &self.rows {
            if ly >= row.y && ly < row.y + row.h {
                return Some(match row.kind {
                    SidebarRowKind::Header { tab_idx, .. } => {
                        if px < self.cell_w * CHEVRON_CELLS {
                            SidebarHit::Chevron(tab_idx)
                        } else if px >= self.width - HEADER_ADD_CELLS * self.cell_w {
                            SidebarHit::HeaderAdd(tab_idx)
                        } else {
                            SidebarHit::Header(tab_idx)
                        }
                    }
                    SidebarRowKind::Pane { pane_id, tile, .. } => {
                        let tile_y = self.list_y + row.y - self.scroll_y;
                        self.tile_hit(pane_id, &tile, tile_y, px, py)
                    }
                });
            }
        }
        Some(SidebarHit::Empty)
    }

    /// What a point inside a tile's row lands on: a glyph box on line 0, the
    /// awaiting tile's action line, the shell tile's call, or the body.
    fn tile_hit(&self, pane_id: PaneId, tile: &TileLayout, tile_y: f32, px: f32, py: f32) -> SidebarHit {
        if px < self.tile_x() || px >= self.tile_x() + self.tile_w() {
            return SidebarHit::Empty;
        }
        let on_line = |k: u8| {
            let y = self.line_y(tile_y, k);
            py >= y && py < y + self.cell_h
        };
        if on_line(0) {
            for (button, x) in self.glyph_boxes(tile) {
                if px >= x && px < x + GLYPH_BOX_CELLS * self.cell_w {
                    return SidebarHit::PaneButton(pane_id, button);
                }
            }
        }
        if tile.awaiting() && on_line(3) {
            let open_w = OPEN_LABEL.chars().count() as f32 * self.cell_w;
            if px >= self.content_x() && px < self.content_x() + open_w {
                return SidebarHit::PaneButton(pane_id, TileButton::Open);
            }
            let stop_w = STOP_LABEL.chars().count() as f32 * self.cell_w;
            if px >= self.content_right() - stop_w && px < self.content_right() {
                return SidebarHit::PaneButton(pane_id, TileButton::Stop);
            }
        }
        if tile.bare_shell && on_line(1) {
            let start_w = START_CLAUDE_LABEL.chars().count() as f32 * self.cell_w;
            if px >= self.content_right() - start_w && px < self.content_right() {
                return SidebarHit::PaneButton(pane_id, TileButton::StartClaude);
            }
        }
        SidebarHit::Pane(pane_id)
    }

    // --- tab drag ---

    /// Where a dragged group would land: the position (0 ..= groups) such
    /// that the cursor is above the vertical centre of the header at that
    /// position and below the one before it.
    pub fn insertion_index(&self, py: f32) -> usize {
        let ly = py - self.list_y + self.scroll_y;
        let mut k = 0;
        for row in &self.rows {
            if let SidebarRowKind::Header { .. } = row.kind {
                if ly < row.y + row.h / 2.0 {
                    return k;
                }
                k += 1;
            }
        }
        k
    }

    /// Screen y of the insertion line for position `k`: the top of the k-th
    /// header, or the bottom of the last group past the end.
    pub fn insertion_line_y(&self, k: usize) -> f32 {
        let mut seen = 0;
        let mut last_bottom = 0.0_f32;
        for row in &self.rows {
            if let SidebarRowKind::Header { .. } = row.kind {
                if seen == k {
                    return self.list_y + row.y - self.scroll_y;
                }
                seen += 1;
            }
            last_bottom = row.y + row.h;
        }
        self.list_y + last_bottom - self.scroll_y
    }

    // --- pane drag ---

    /// The contiguous run of rows (`start..end`) listing the panes of the
    /// same column of the same tab as row `idx`: the only slots a dragged
    /// tile can take.
    pub fn pane_run(&self, idx: usize) -> std::ops::Range<usize> {
        let SidebarRowKind::Pane { tab_idx, column, .. } = self.rows[idx].kind else {
            return idx..idx;
        };
        let same = |r: &SidebarRow| matches!(r.kind, SidebarRowKind::Pane { tab_idx: t, column: c, .. } if t == tab_idx && c == column);
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
    pub fn pane_insertion_slot(&self, run: &std::ops::Range<usize>, py: f32) -> Option<usize> {
        let ly = py - self.list_y + self.scroll_y;
        let first = &self.rows[run.start];
        let last = &self.rows[run.end - 1];
        if ly < first.y - self.cell_h || ly >= last.y + last.h + self.cell_h {
            return None;
        }
        for (k, row) in self.rows[run.clone()].iter().enumerate() {
            if ly < row.y + row.h / 2.0 {
                return Some(k);
            }
        }
        Some(run.len())
    }

    /// Screen y of the insertion line for `slot` in `run`: the gap above the
    /// slot's tile, or below the last one.
    pub fn pane_insertion_line_y(&self, run: &std::ops::Range<usize>, slot: usize) -> f32 {
        let half_gap = (self.cell_h * 0.25).round();
        if slot < run.len() {
            self.row_screen_y(run.start + slot) - half_gap
        } else {
            let last = &self.rows[run.end - 1];
            self.list_y + last.y + last.h - self.scroll_y + half_gap
        }
    }

    /// Whether the cursor is close enough to the list's top (`-1`) or bottom
    /// (`1`) edge for a drag to auto-scroll, `0` otherwise.
    pub fn autoscroll_direction(&self, py: f32) -> i32 {
        let margin = self.cell_h * 1.5;
        if py < self.list_y + margin {
            -1
        } else if py > self.list_y + self.list_h - margin {
            1
        } else {
            0
        }
    }
}

/// The adjacent swaps that move the item at `from` to `to` in a list of
/// `len`, keeping every other item in order: what a pane drop replays with
/// `Tab::swap_panes`. Each pair is (index, index) before that swap.
pub fn swap_chain(len: usize, from: usize, to: usize) -> Vec<(usize, usize)> {
    if from >= len || to >= len {
        return Vec::new();
    }
    if from < to {
        (from..to).map(|i| (i, i + 1)).collect()
    } else {
        (to..from).rev().map(|i| (i + 1, i)).collect()
    }
}

/// The index a dragged item ends at when dropped in `slot` (0 ..= len) of a
/// list it already belongs to at `from`.
pub fn drop_index(from: usize, slot: usize) -> usize {
    if slot > from { slot - 1 } else { slot }
}

// ---------------------------------------------------------------
// Summary and Next pill
// ---------------------------------------------------------------

/// One coloured run of the summary line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryRun {
    Waiting,
    Working,
    Idle,
    Dot,
}

/// `2 waiting · 3 working · 4 idle`, each count its own run so the renderer
/// colours them apart; zero counts are left out; `nothing running` when all
/// three are zero.
pub fn summary_runs(waiting: usize, working: usize, idle: usize) -> Vec<(String, SummaryRun)> {
    let mut runs = Vec::new();
    let mut push = |text: String, run: SummaryRun| {
        if !runs.is_empty() {
            runs.push((" \u{b7} ".to_string(), SummaryRun::Dot));
        }
        runs.push((text, run));
    };
    if waiting > 0 {
        push(format!("{waiting} waiting"), SummaryRun::Waiting);
    }
    if working > 0 {
        push(format!("{working} working"), SummaryRun::Working);
    }
    if idle > 0 {
        push(format!("{idle} idle"), SummaryRun::Idle);
    }
    if runs.is_empty() {
        runs.push(("nothing running".to_string(), SummaryRun::Idle));
    }
    runs
}

/// What the Next pill shows (KovaLink `NextPill.tsx`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextPill {
    /// Unread panes left: accent pill, count badge.
    Next(usize),
    /// Nothing unread, idle sessions left: neutral pill, count badge.
    Idle(usize),
    /// The last unread was just read: a short green flash.
    CaughtUp,
    /// Nothing to read at all: dim, not clickable.
    Nothing,
}

impl NextPill {
    pub fn of(unread: usize, idle: usize, flashing: bool) -> Self {
        if unread > 0 {
            NextPill::Next(unread)
        } else if idle > 0 {
            NextPill::Idle(idle)
        } else if flashing {
            NextPill::CaughtUp
        } else {
            NextPill::Nothing
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            NextPill::Next(_) => "\u{25b6} Next unread",
            NextPill::Idle(_) => "\u{25b6} Next idle",
            NextPill::CaughtUp => "\u{2713} All caught up",
            NextPill::Nothing => "\u{2713} Nothing to read",
        }
    }

    pub fn badge(self) -> Option<usize> {
        match self {
            NextPill::Next(n) | NextPill::Idle(n) => Some(n),
            _ => None,
        }
    }

    pub fn clickable(self) -> bool {
        matches!(self, NextPill::Next(_) | NextPill::Idle(_))
    }
}

/// `4s`, `12m`, `3h`: how long a pane has been waiting.
pub fn format_age(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

/// Seconds after which a waiting pane's age turns red (KovaLink `aging`).
pub const AGING_SECS: u64 = 600;

// ---------------------------------------------------------------
// Text rules (char based, never byte slices)
// ---------------------------------------------------------------

/// Keep a title inside `n` cells: as is when it fits, else its first `n - 1`
/// chars and an ellipsis.
pub fn truncate_title(title: &str, n: usize) -> String {
    let count = title.chars().count();
    if count <= n {
        return title.to_string();
    }
    if n == 0 {
        return String::new();
    }
    let mut out: String = title.chars().take(n - 1).collect();
    out.push('\u{2026}');
    out
}

/// Keep a path inside `n` cells from the right: an ellipsis and its last
/// chars, moved forward to the next `/` so the line starts on a segment.
pub fn truncate_path(path: &str, n: usize) -> String {
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= n {
        return path.to_string();
    }
    if n == 0 {
        return String::new();
    }
    let mut start = chars.len() - (n - 1);
    if chars[start] != '/' {
        if let Some(next_slash) = chars[start..].iter().position(|&c| c == '/') {
            start += next_slash;
        }
    }
    let mut out = String::from("\u{2026}");
    out.extend(&chars[start..]);
    out
}

/// The end of an edit buffer, so the cursor glyph stays visible while a
/// name is being typed (same rule as the tab bar's rename branch).
pub fn edit_tail(text: &str, n: usize) -> String {
    let count = text.chars().count();
    if count <= n {
        return text.to_string();
    }
    text.chars().skip(count - n).collect()
}

/// A working directory with `$HOME` folded to `~`.
pub fn short_cwd(cwd: &str, home: &str) -> String {
    if !home.is_empty() {
        if cwd == home {
            return "~".to_string();
        }
        if let Some(rest) = cwd.strip_prefix(home) {
            if rest.starts_with('/') {
                return format!("~{rest}");
            }
        }
    }
    cwd.to_string()
}

/// The second line of a pane tile: what runs in it, then where.
pub fn secondary_line(agent: Option<&str>, process: Option<&str>, cwd_short: &str) -> String {
    match agent.or(process) {
        Some(what) if !cwd_short.is_empty() => format!("{what} \u{b7} {cwd_short}"),
        Some(what) => what.to_string(),
        None => cwd_short.to_string(),
    }
}

/// Word-wrap `text` into at most `max_lines` lines of `cells` chars; the
/// last line ends with an ellipsis when something was cut. A word longer
/// than a line is split.
pub fn wrap_text(text: &str, cells: usize, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    if cells == 0 || max_lines == 0 {
        return lines;
    }
    let mut current: Vec<char> = Vec::new();
    let mut cut = false;
    'words: for word in text.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        while !word.is_empty() {
            let need = if current.is_empty() { 0 } else { 1 } + word.len();
            if current.len() + need <= cells {
                if !current.is_empty() {
                    current.push(' ');
                }
                current.extend(word.drain(..));
            } else if current.is_empty() {
                // The word alone is too long: take what fits.
                current.extend(word.drain(..cells));
            } else {
                if lines.len() + 1 == max_lines {
                    cut = true;
                    break 'words;
                }
                lines.push(current.iter().collect());
                current.clear();
            }
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current.iter().collect());
    }
    if cut {
        let last = lines.last_mut().expect("one line");
        let chars: Vec<char> = last.chars().collect();
        let keep = chars.len().min(cells - 1);
        *last = chars[..keep].iter().collect::<String>() + "\u{2026}";
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: (f32, f32) = (16.0, 32.0);

    fn header(tab_idx: usize, collapsed: bool) -> SidebarRowKind {
        SidebarRowKind::Header { tab_idx, collapsed }
    }

    fn tile(state: TileState, lines: u8) -> TileLayout {
        TileLayout { state, minimized: false, bare_shell: state == TileState::Shell, lines }
    }

    fn pane(tab_idx: usize, pane_id: PaneId) -> SidebarRowKind {
        SidebarRowKind::Pane { tab_idx, pane_id, column: 0, tile: tile(TileState::Idle, 2) }
    }

    fn pane_in(tab_idx: usize, pane_id: PaneId, column: usize, t: TileLayout) -> SidebarRowKind {
        SidebarRowKind::Pane { tab_idx, pane_id, column, tile: t }
    }

    /// Two groups: tab 0 expanded with two panes, tab 1 collapsed, tab 2
    /// expanded with one pane.
    fn kinds() -> Vec<SidebarRowKind> {
        vec![header(0, false), pane(0, 10), pane(0, 11), header(1, true), header(2, false), pane(2, 30)]
    }

    fn geometry(height: f32, scroll: f32) -> SidebarGeometry {
        SidebarGeometry::new(CELL, 2.0, 32, height, scroll, &kinds(), false)
    }

    /// list_y = 2 + 1.5 + 2 ch = 176 px.
    const LIST_Y: f32 = 176.0;

    #[test]
    fn rows_stack_with_tile_gaps_and_a_group_gap_after_expanded_groups() {
        let g = geometry(2000.0, 0.0);
        assert_eq!(g.header_h, 64.0);
        let ys: Vec<f32> = g.rows.iter().map(|r| r.y).collect();
        // header 0..64, gap 16, tile 80..176, gap 16, tile 192..288, group
        // gap 32, header 320..384 (collapsed: no gap), header 384..448, gap
        // 16, tile 464..560, trailing group gap 32.
        assert_eq!(ys, vec![0.0, 80.0, 192.0, 320.0, 384.0, 464.0]);
        assert_eq!(g.rows[1].h, 96.0);
        assert_eq!(g.content_h, 592.0);
        assert_eq!(g.width, 32.0 * 16.0);
        assert_eq!(g.sep_w, 2.0);
        assert_eq!(g.list_y, LIST_Y);
        assert_eq!(g.list_h, 2000.0 - LIST_Y - 48.0);
        assert_eq!((g.pill_y, g.pill_h), (64.0 + 48.0 + 8.0, 48.0));
    }

    #[test]
    fn tile_height_follows_its_line_count() {
        let kinds = vec![
            header(0, false),
            pane_in(0, 1, 0, tile(TileState::Awaiting, 4)),
            pane_in(0, 2, 0, tile(TileState::Unread { bell: false }, 3)),
            pane_in(0, 3, 0, tile(TileState::Working, 2)),
        ];
        let g = SidebarGeometry::new(CELL, 2.0, 32, 2000.0, 0.0, &kinds, false);
        let hs: Vec<f32> = g.rows.iter().map(|r| r.h).collect();
        assert_eq!(hs, vec![64.0, 160.0, 128.0, 96.0]);
        assert_eq!(g.line_y(100.0, 0), 116.0);
        assert_eq!(g.line_y(100.0, 3), 212.0);
    }

    #[test]
    fn the_hint_takes_a_line_below_the_last_group() {
        let g = SidebarGeometry::new(CELL, 2.0, 32, 2000.0, 0.0, &kinds(), true);
        assert_eq!(g.hint_y, Some(592.0));
        assert_eq!(g.content_h, 592.0 + 64.0);
        assert_eq!(g.hint_screen_y(), Some(LIST_Y + 592.0));
    }

    #[test]
    fn width_is_snapped_into_range() {
        let g = SidebarGeometry::new(CELL, 2.0, 4, 1000.0, 0.0, &[], false);
        assert_eq!(g.width_cells, 22);
        let g = SidebarGeometry::new(CELL, 2.0, 99, 1000.0, 0.0, &[], false);
        assert_eq!(g.width_cells, 56);
    }

    #[test]
    fn tile_frame_is_inset_one_cell_with_a_half_cell_bar() {
        let g = geometry(2000.0, 0.0);
        assert_eq!(g.tile_x(), 16.0);
        assert_eq!(g.tile_w(), 480.0);
        assert_eq!(g.bar_w(), 8.0);
        assert_eq!(g.content_x(), 40.0);
        assert_eq!(g.content_right(), 480.0);
        assert_eq!(g.content_cells(), 27);
        assert_eq!(g.title_cells_beside(9), 17);
    }

    #[test]
    fn hit_tells_the_regions_apart() {
        let g = geometry(2000.0, 0.0);
        assert_eq!(g.hit(10.0, 10.0), Some(SidebarHit::TopArea));
        // Summary row: sort toggle on the right, nothing on the left.
        assert_eq!(g.hit(10.0, 70.0), Some(SidebarHit::Empty));
        assert_eq!(g.hit(g.width - 20.0, 70.0), Some(SidebarHit::SortToggle));
        // Pill region: the pill itself, and the margins around it.
        assert_eq!(g.hit(100.0, 130.0), Some(SidebarHit::NextPill));
        assert_eq!(g.hit(100.0, 114.0), Some(SidebarHit::Empty));
        assert_eq!(g.hit(4.0, 130.0), Some(SidebarHit::Empty));
        // Header: chevron, body, `+`.
        assert_eq!(g.hit(10.0, LIST_Y + 10.0), Some(SidebarHit::Chevron(0)));
        assert_eq!(g.hit(100.0, LIST_Y + 10.0), Some(SidebarHit::Header(0)));
        assert_eq!(g.hit(g.width - 30.0, LIST_Y + 10.0), Some(SidebarHit::HeaderAdd(0)));
        // Tiles: the gap above a tile is nothing, the body is the pane.
        assert_eq!(g.hit(100.0, LIST_Y + 70.0), Some(SidebarHit::Empty));
        assert_eq!(g.hit(100.0, LIST_Y + 150.0), Some(SidebarHit::Pane(10)));
        assert_eq!(g.hit(100.0, LIST_Y + 250.0), Some(SidebarHit::Pane(11)));
        // Outside the tile frame, inside its row: nothing.
        assert_eq!(g.hit(4.0, LIST_Y + 150.0), Some(SidebarHit::Empty));
        // The group gap, then the collapsed header, then tab 2's tile.
        assert_eq!(g.hit(100.0, LIST_Y + 300.0), Some(SidebarHit::Empty));
        assert_eq!(g.hit(100.0, LIST_Y + 330.0), Some(SidebarHit::Header(1)));
        assert_eq!(g.hit(100.0, LIST_Y + 500.0), Some(SidebarHit::Pane(30)));
        // Below the content, still in the list.
        assert_eq!(g.hit(100.0, 1000.0), Some(SidebarHit::Empty));
        // Footer: mode button on the right.
        assert_eq!(g.hit(g.width - 20.0, 2000.0 - 10.0), Some(SidebarHit::ModeButton));
        assert_eq!(g.hit(10.0, 2000.0 - 10.0), Some(SidebarHit::Empty));
        // Right of the separator is not ours.
        assert_eq!(g.hit(g.total_w() + 20.0, 500.0), None);
        assert_eq!(g.hit(100.0, 2500.0), None);
    }

    #[test]
    fn glyph_boxes_sit_on_line_zero_from_the_right() {
        let kinds = vec![
            header(0, false),
            pane_in(0, 1, 0, TileLayout { state: TileState::Working, minimized: false, bare_shell: false, lines: 2 }),
        ];
        let g = SidebarGeometry::new(CELL, 2.0, 32, 2000.0, 0.0, &kinds, false);
        let tile_y = LIST_Y + 80.0;
        let line0 = g.line_y(tile_y, 0);
        // Working: close at 432..480, minimize at 384..432, stop at 336..384.
        assert_eq!(g.hit(470.0, line0 + 5.0), Some(SidebarHit::PaneButton(1, TileButton::Close)));
        assert_eq!(g.hit(400.0, line0 + 5.0), Some(SidebarHit::PaneButton(1, TileButton::Minimize)));
        assert_eq!(g.hit(340.0, line0 + 5.0), Some(SidebarHit::PaneButton(1, TileButton::Stop)));
        assert_eq!(g.hit(300.0, line0 + 5.0), Some(SidebarHit::Pane(1)));
        // Line 1 carries no glyphs.
        assert_eq!(g.hit(470.0, g.line_y(tile_y, 1) + 5.0), Some(SidebarHit::Pane(1)));
        let boxes = g.glyph_boxes(&TileLayout { state: TileState::Idle, minimized: true, bare_shell: false, lines: 2 });
        assert_eq!(boxes, vec![(TileButton::Close, 432.0), (TileButton::Restore, 384.0)]);
    }

    #[test]
    fn the_awaiting_tile_has_open_and_stop_on_its_last_line() {
        let kinds = vec![header(0, false), pane_in(0, 1, 0, tile(TileState::Awaiting, 4))];
        let g = SidebarGeometry::new(CELL, 2.0, 32, 2000.0, 0.0, &kinds, false);
        let tile_y = LIST_Y + 80.0;
        let line3 = g.line_y(tile_y, 3) + 5.0;
        assert_eq!(g.hit(45.0, line3), Some(SidebarHit::PaneButton(1, TileButton::Open)));
        assert_eq!(g.hit(400.0, line3), Some(SidebarHit::PaneButton(1, TileButton::Stop)));
        assert_eq!(g.hit(200.0, line3), Some(SidebarHit::Pane(1)));
        // Its hover glyphs include stop.
        assert_eq!(tile(TileState::Awaiting, 4).glyphs(), vec![TileButton::Close, TileButton::Minimize, TileButton::Stop]);
    }

    #[test]
    fn the_bare_shell_tile_offers_start_claude_on_line_one() {
        let kinds = vec![header(0, false), pane_in(0, 1, 0, tile(TileState::Shell, 2))];
        let g = SidebarGeometry::new(CELL, 2.0, 32, 2000.0, 0.0, &kinds, false);
        let tile_y = LIST_Y + 80.0;
        let line1 = g.line_y(tile_y, 1) + 5.0;
        // "▶ Start Claude" is 14 cells: 256..480.
        assert_eq!(g.hit(300.0, line1), Some(SidebarHit::PaneButton(1, TileButton::StartClaude)));
        assert_eq!(g.hit(100.0, line1), Some(SidebarHit::Pane(1)));
        assert_eq!(tile(TileState::Shell, 2).glyphs(), vec![TileButton::Close, TileButton::Minimize, TileButton::StartClaude]);
        let mut t = tile(TileState::Shell, 2);
        t.bare_shell = false;
        assert_eq!(t.glyphs(), vec![TileButton::Close, TileButton::Minimize]);
    }

    #[test]
    fn the_resize_handle_wins_near_the_separator() {
        let g = geometry(2000.0, 0.0);
        let edge = g.width + g.sep_w / 2.0;
        assert_eq!(g.hit(edge - 7.0, 500.0), Some(SidebarHit::Edge));
        assert_eq!(g.hit(edge + 7.0, 500.0), Some(SidebarHit::Edge));
        // 4pt at 2x is 8px of tolerance either side; inside it, the row's
        // margin (the tile frame stops one cell short of the edge).
        assert_eq!(g.hit(edge - 9.0, LIST_Y + 250.0), Some(SidebarHit::Empty));
        assert_eq!(g.hit(edge + 9.0, 500.0), None);
    }

    #[test]
    fn scrolling_shifts_what_a_point_hits() {
        // A 200px list over 592px of rows, scrolled 100px down.
        let g = geometry(LIST_Y + 200.0 + 48.0, 100.0);
        assert_eq!(g.scroll_y, 100.0);
        // Content y 110: tile 10 (80..176).
        assert_eq!(g.hit(100.0, LIST_Y + 10.0), Some(SidebarHit::Pane(10)));
        // Content y 230: tile 11 (192..288).
        assert_eq!(g.hit(100.0, LIST_Y + 130.0), Some(SidebarHit::Pane(11)));
    }

    #[test]
    fn scroll_is_clamped_to_the_overflow() {
        // A list 200px tall holding 592px of rows can scroll 392px at most.
        let g = geometry(LIST_Y + 200.0 + 48.0, 999.0);
        assert_eq!(g.max_scroll(), 392.0);
        assert_eq!(g.scroll_y, 392.0);
        assert_eq!(g.overflow(), (true, false));
        let g = geometry(LIST_Y + 200.0 + 48.0, 0.0);
        assert_eq!(g.overflow(), (false, true));
        // Everything fits: no scroll at all.
        let g = geometry(2000.0, 50.0);
        assert_eq!(g.scroll_y, 0.0);
        assert_eq!(g.overflow(), (false, false));
    }

    #[test]
    fn reveal_moves_the_least_that_shows_the_whole_row() {
        let g = geometry(LIST_Y + 200.0 + 48.0, 0.0);
        // Row 2 (tile 11, 192..288) sticks out below a 200px list.
        assert_eq!(g.reveal(2), 88.0);
        // Row 0 is already visible.
        assert_eq!(g.reveal(0), 0.0);
        let g = geometry(LIST_Y + 200.0 + 48.0, 150.0);
        // Row 0 (0..64) is above: scroll back to its top.
        assert_eq!(g.reveal(0), 0.0);
        // Row 5 (464..560) sits below 150..350: bring its bottom to 350.
        assert_eq!(g.reveal(5), 360.0);
        assert_eq!(g.row_for_pane(30), Some(5));
        assert_eq!(g.row_for_pane(99), None);
        assert_eq!(g.row_for_header(1), Some(3));
    }

    #[test]
    fn insertion_index_follows_the_midpoint_rule() {
        let g = geometry(2000.0, 0.0);
        // Above the centre of header 0 (32): before it.
        assert_eq!(g.insertion_index(LIST_Y + 10.0), 0);
        // Below its centre: before header 1.
        assert_eq!(g.insertion_index(LIST_Y + 100.0), 1);
        // Header 1 spans 320..384, centre 352.
        assert_eq!(g.insertion_index(LIST_Y + 340.0), 1);
        assert_eq!(g.insertion_index(LIST_Y + 360.0), 2);
        // Past the last header's centre (416): at the end.
        assert_eq!(g.insertion_index(LIST_Y + 420.0), 3);
        assert_eq!(g.insertion_index(1500.0), 3);
        assert_eq!(g.insertion_line_y(0), LIST_Y);
        assert_eq!(g.insertion_line_y(1), LIST_Y + 320.0);
        assert_eq!(g.insertion_line_y(3), LIST_Y + 560.0);
    }

    #[test]
    fn a_pane_drag_stays_inside_its_column_run() {
        // Tab 0: column 0 holds panes 1, 2; column 1 holds pane 3. Tab 1: pane 4.
        let kinds = vec![
            header(0, false),
            pane_in(0, 1, 0, tile(TileState::Idle, 2)),
            pane_in(0, 2, 0, tile(TileState::Idle, 2)),
            pane_in(0, 3, 1, tile(TileState::Idle, 2)),
            header(1, false),
            pane_in(1, 4, 0, tile(TileState::Idle, 2)),
        ];
        let g = SidebarGeometry::new(CELL, 2.0, 32, 2000.0, 0.0, &kinds, false);
        assert_eq!(g.pane_run(1), 1..3);
        assert_eq!(g.pane_run(2), 1..3);
        assert_eq!(g.pane_run(3), 3..4);
        assert_eq!(g.pane_run(5), 5..6);
        assert_eq!(g.pane_run(0), 0..0);
        let run = 1..3;
        // Tile 1 spans 80..176 (centre 128), tile 2 spans 192..288 (centre 240).
        assert_eq!(g.pane_insertion_slot(&run, LIST_Y + 100.0), Some(0));
        assert_eq!(g.pane_insertion_slot(&run, LIST_Y + 200.0), Some(1));
        assert_eq!(g.pane_insertion_slot(&run, LIST_Y + 280.0), Some(2));
        // Far above or below the run: no slot, the drop snaps back.
        assert_eq!(g.pane_insertion_slot(&run, LIST_Y + 10.0), None);
        assert_eq!(g.pane_insertion_slot(&run, LIST_Y + 400.0), None);
        assert_eq!(g.pane_insertion_line_y(&run, 0), LIST_Y + 80.0 - 8.0);
        assert_eq!(g.pane_insertion_line_y(&run, 1), LIST_Y + 192.0 - 8.0);
        assert_eq!(g.pane_insertion_line_y(&run, 2), LIST_Y + 288.0 + 8.0);
    }

    #[test]
    fn a_drop_replays_as_adjacent_swaps() {
        assert_eq!(swap_chain(4, 0, 2), vec![(0, 1), (1, 2)]);
        assert_eq!(swap_chain(4, 3, 1), vec![(3, 2), (2, 1)]);
        assert_eq!(swap_chain(4, 2, 2), Vec::<(usize, usize)>::new());
        assert_eq!(swap_chain(2, 5, 0), Vec::<(usize, usize)>::new());
        // Slot arithmetic: dropping below itself shifts by one.
        assert_eq!(drop_index(0, 2), 1);
        assert_eq!(drop_index(0, 1), 0);
        assert_eq!(drop_index(2, 0), 0);
        assert_eq!(drop_index(1, 3), 2);
    }

    #[test]
    fn autoscroll_zones_hug_the_list_edges() {
        let g = geometry(2000.0, 0.0);
        assert_eq!(g.autoscroll_direction(g.list_y + 10.0), -1);
        assert_eq!(g.autoscroll_direction(g.list_y + 100.0), 0);
        assert_eq!(g.autoscroll_direction(g.list_y + g.list_h - 10.0), 1);
    }

    #[test]
    fn title_cells_leave_room_for_the_plus_and_the_chips() {
        let g = geometry(2000.0, 0.0);
        assert_eq!(g.title_cells(false), 32 - 9);
        assert_eq!(g.title_cells(true), 32 - 17);
        assert_eq!(g.header_add_x(), 464.0);
        assert_eq!(g.header_summary_right(), 448.0);
    }

    #[test]
    fn titles_truncate_from_the_right_with_an_ellipsis() {
        assert_eq!(truncate_title("fix-voice-input", 20), "fix-voice-input");
        assert_eq!(truncate_title("fix-voice-input", 8), "fix-voi\u{2026}");
        assert_eq!(truncate_title("émoji ✳ title", 6), "émoji\u{2026}");
        assert_eq!(truncate_title("abc", 0), "");
    }

    #[test]
    fn paths_truncate_from_the_left_on_a_segment_boundary() {
        assert_eq!(truncate_path("~/link", 10), "~/link");
        assert_eq!(
            truncate_path("~/AI directory/Claap/Product/personal-tools/kova", 24),
            "\u{2026}/personal-tools/kova"
        );
        // A cut landing right after a slash keeps that segment whole.
        assert_eq!(truncate_path("/a/bb/ccc", 5), "\u{2026}/ccc");
        // No later slash: the tail as is.
        assert_eq!(truncate_path("abcdefgh", 4), "\u{2026}fgh");
    }

    #[test]
    fn edit_tail_keeps_the_cursor_end_of_a_long_name() {
        assert_eq!(edit_tail("short\u{258f}", 10), "short\u{258f}");
        assert_eq!(edit_tail("a very long tab name\u{258f}", 5), "name\u{258f}");
    }

    #[test]
    fn cwd_folds_home_and_leaves_lookalikes_alone() {
        assert_eq!(short_cwd("/Users/rob/link", "/Users/rob"), "~/link");
        assert_eq!(short_cwd("/Users/rob", "/Users/rob"), "~");
        assert_eq!(short_cwd("/Users/robert/x", "/Users/rob"), "/Users/robert/x");
        assert_eq!(short_cwd("/tmp", ""), "/tmp");
    }

    #[test]
    fn secondary_line_names_the_agent_before_the_process() {
        assert_eq!(secondary_line(Some("claude"), Some("node"), "~/link"), "claude \u{b7} ~/link");
        assert_eq!(secondary_line(None, Some("nvim"), "~/link"), "nvim \u{b7} ~/link");
        assert_eq!(secondary_line(None, None, "~/link"), "~/link");
        assert_eq!(secondary_line(Some("codex"), None, ""), "codex");
    }

    #[test]
    fn questions_wrap_on_words_and_end_with_an_ellipsis_when_cut() {
        assert_eq!(wrap_text("Do you want to proceed?", 30, 2), vec!["Do you want to proceed?"]);
        assert_eq!(
            wrap_text("Should I overwrite hello.txt with the new content?", 28, 2),
            vec!["Should I overwrite hello.txt", "with the new content?"]
        );
        let cut = wrap_text("one two three four five six seven eight nine ten", 12, 2);
        assert_eq!(cut, vec!["one two", "three four\u{2026}"]);
        assert!(cut.iter().all(|l| l.chars().count() <= 12));
        // A word longer than the line is split rather than dropped.
        assert_eq!(wrap_text("abcdefghij", 4, 3), vec!["abcd", "efgh", "ij"]);
        assert_eq!(wrap_text("", 10, 2), vec![""]);
        assert!(wrap_text("x", 0, 2).is_empty());
    }

    #[test]
    fn summary_runs_only_mention_what_is_there() {
        let text = |runs: Vec<(String, SummaryRun)>| runs.into_iter().map(|(t, _)| t).collect::<String>();
        assert_eq!(text(summary_runs(0, 0, 0)), "nothing running");
        assert_eq!(text(summary_runs(1, 0, 0)), "1 waiting");
        assert_eq!(text(summary_runs(0, 2, 4)), "2 working \u{b7} 4 idle");
        let runs = summary_runs(1, 2, 3);
        assert_eq!(text(runs.clone()), "1 waiting \u{b7} 2 working \u{b7} 3 idle");
        assert_eq!(runs[0].1, SummaryRun::Waiting);
        assert_eq!(runs[1].1, SummaryRun::Dot);
        assert_eq!(runs[2].1, SummaryRun::Working);
        assert_eq!(runs[4].1, SummaryRun::Idle);
    }

    #[test]
    fn the_next_pill_prefers_unread_then_idle_then_the_flash() {
        assert_eq!(NextPill::of(3, 2, false), NextPill::Next(3));
        assert_eq!(NextPill::of(0, 2, true), NextPill::Idle(2));
        assert_eq!(NextPill::of(0, 0, true), NextPill::CaughtUp);
        assert_eq!(NextPill::of(0, 0, false), NextPill::Nothing);
        assert_eq!(NextPill::Next(3).badge(), Some(3));
        assert_eq!(NextPill::CaughtUp.badge(), None);
        assert!(NextPill::Idle(1).clickable());
        assert!(!NextPill::Nothing.clickable());
        assert_eq!(NextPill::Next(1).label(), "\u{25b6} Next unread");
    }

    #[test]
    fn ages_read_as_seconds_minutes_or_hours() {
        assert_eq!(format_age(4), "4s");
        assert_eq!(format_age(59), "59s");
        assert_eq!(format_age(60), "1m");
        assert_eq!(format_age(4 * 60 + 12), "4m");
        assert_eq!(format_age(3 * 3600 + 5), "3h");
    }

    #[test]
    fn tile_state_follows_the_priority_order() {
        use TileState::*;
        let all = PaneFlags {
            permission_prompt: true,
            bell: true,
            completion: true,
            hook_unseen: true,
            turn_end_unseen: true,
            working: true,
            starting: true,
            idle_agent: true,
            seen: false,
        };
        // A prompt on screen loses to the spinner: Claude went back to work.
        assert_eq!(TileState::from_flags(all), Unread { bell: false });
        assert_eq!(TileState::from_flags(PaneFlags { working: false, ..all }), Awaiting);
        assert_eq!(TileState::from_flags(PaneFlags { permission_prompt: false, completion: false, ..all }), Unread { bell: true });
        // The hook flag alone paints done, never waiting.
        assert_eq!(TileState::from_flags(PaneFlags { hook_unseen: true, ..PaneFlags::default() }), Unread { bell: false });
        assert_eq!(TileState::from_flags(PaneFlags { turn_end_unseen: true, ..PaneFlags::default() }), Unread { bell: false });
        // The pane being looked at never shows unread.
        assert_eq!(TileState::from_flags(PaneFlags { permission_prompt: false, seen: true, ..all }), Working);
        assert_eq!(TileState::from_flags(PaneFlags { starting: true, idle_agent: true, ..PaneFlags::default() }), Starting);
        assert_eq!(TileState::from_flags(PaneFlags { idle_agent: true, ..PaneFlags::default() }), Idle);
        assert_eq!(TileState::from_flags(PaneFlags::default()), Shell);
        assert!(Idle.neutral() && Shell.neutral() && !Working.neutral());
        assert_eq!(Unread { bell: true }.chip(), "\u{25cf} bell");
        assert_eq!(Working.color(), tokens::WORKING);
        assert_eq!(Shell.color(), tokens::BORDER_STRONG);
    }

    #[test]
    fn a_collapsed_group_counts_waiting_and_working_panes() {
        use TileState::*;
        let s = CollapsedSummary::of([Working, Awaiting, Shell, Working].into_iter());
        assert_eq!(s, CollapsedSummary { awaiting: 1, working: 2, count: 4 });
        assert_eq!(s.count_label(), "4 panes");
        // `[1]` + gap + `[2]` + gap + `4 panes` = 4 + 4 + 7.
        assert_eq!(s.cells(), 15);
        let one = CollapsedSummary::of([Idle].into_iter());
        assert_eq!(one.count_label(), "1 pane");
        assert_eq!(one.cells(), 6);
        assert_eq!(TileState::most_urgent([Working, Awaiting, Shell].into_iter()), Awaiting);
        assert_eq!(TileState::most_urgent(std::iter::empty()), Shell);
    }

    #[test]
    fn activity_sort_puts_urgent_tabs_first_and_keeps_ties_in_tab_order() {
        use TileState::*;
        let states = [Shell, Working, Awaiting, Working, Idle];
        assert_eq!(display_order(SidebarSort::Kova, &states), vec![0, 1, 2, 3, 4]);
        assert_eq!(display_order(SidebarSort::Activity, &states), vec![2, 1, 3, 4, 0]);
        assert_eq!(SidebarSort::Kova.toggled(), SidebarSort::Activity);
        assert_eq!(SidebarSort::Activity.label(), "\u{21c5} activity");
    }

    #[test]
    fn band_text_is_dark_on_light_colours_and_tints_sit_near_the_ground() {
        assert_eq!(on_band([0.85, 0.75, 0.15]), tokens::TEXT_INVERSE);
        assert_eq!(on_band([0.82, 0.22, 0.22]), tokens::TEXT_ON_FILL);
        let t = band_tint([0.5, 0.5, 0.5]);
        assert!(t[0] > tokens::GROUND[0] && t[0] < 0.2);
    }
}
