//! The sidebar layout mode: the tabs of a window as collapsible groups down
//! the left edge, one row per pane, instead of the strip across the top.
//!
//! This module holds everything about it that can be checked without a
//! window: the process-wide layout setting and its persistence, the row
//! layout and hit-testing (`SidebarGeometry`), the state-square vocabulary,
//! the activity sort, and the text rules. The renderer paints from the same
//! geometry the mouse handlers consult, so a click lands on the row that was
//! drawn. See `docs/sidebar-spec.md`.

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
static WIDTH_CELLS: AtomicU16 = AtomicU16::new(28);

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
            SidebarSort::Kova => "kova",
            SidebarSort::Activity => "activity",
        }
    }
}

/// The order tabs are listed in: their own order, or by the most urgent state
/// of any of their panes (`tab_states[i]` is that state for tab `i`), ties
/// keeping the tab order so nothing jumps around while states are equal.
pub fn display_order(sort: SidebarSort, tab_states: &[StateSquare]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..tab_states.len()).collect();
    if sort == SidebarSort::Activity {
        order.sort_by_key(|&i| tab_states[i]);
    }
    order
}

// ---------------------------------------------------------------
// State squares
// ---------------------------------------------------------------

/// What a pane row's square says, in priority order: the first variant that
/// applies wins, and the order doubles as urgency for the activity sort.
/// Bell and completion are only meaningful on panes nobody is looking at;
/// callers pass `seen` for the focused pane of the active tab so those two
/// never paint there (their flags clear on focus anyway).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StateSquare {
    Awaiting,
    Bell,
    Completion,
    Working,
    IdleAgent,
    None,
}

impl StateSquare {
    pub fn from_flags(
        awaiting: bool,
        bell: bool,
        completion: bool,
        working: bool,
        idle_agent: bool,
        seen: bool,
    ) -> Self {
        if awaiting {
            StateSquare::Awaiting
        } else if bell && !seen {
            StateSquare::Bell
        } else if completion && !seen {
            StateSquare::Completion
        } else if working {
            StateSquare::Working
        } else if idle_agent {
            StateSquare::IdleAgent
        } else {
            StateSquare::None
        }
    }

    /// Fill colour of the square, `None` for a shell without an agent (the
    /// slot stays empty).
    pub fn color(self) -> Option<[f32; 3]> {
        match self {
            StateSquare::Awaiting => Some(AWAITING_COLOR),
            StateSquare::Bell => Some([1.0, 0.45, 0.10]),
            StateSquare::Completion => Some([0.20, 0.80, 0.30]),
            StateSquare::Working => Some(WORKING_COLOR),
            StateSquare::IdleAgent => Some([0.50, 0.50, 0.55]),
            StateSquare::None => None,
        }
    }

    /// An idle agent is drawn as an outline, everything else filled.
    pub fn hollow(self) -> bool {
        self == StateSquare::IdleAgent
    }

    /// The square a collapsed header shows for its whole group: amber if any
    /// pane is waiting, blue if any is working, nothing otherwise.
    pub fn group_summary(states: impl Iterator<Item = StateSquare>) -> StateSquare {
        let mut any_working = false;
        for s in states {
            match s {
                StateSquare::Awaiting => return StateSquare::Awaiting,
                StateSquare::Working => any_working = true,
                _ => {}
            }
        }
        if any_working { StateSquare::Working } else { StateSquare::None }
    }
}

/// KovaLink's `status.awaiting` token, so both screens speak the same colour.
pub const AWAITING_COLOR: [f32; 3] = [1.00, 0.69, 0.13];
/// KovaLink's `status.working` token.
pub const WORKING_COLOR: [f32; 3] = [0.22, 0.74, 0.97];

// ---------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarRowKind {
    Header { tab_idx: usize, collapsed: bool },
    Pane { tab_idx: usize, pane_id: PaneId },
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
    Pane(PaneId),
    SortToggle,
    ModeButton,
    /// The resize handle at the right edge.
    Edge,
    /// The traffic-light strip: window drag region.
    TopArea,
    Empty,
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
    pub header_h: f32,
    pub row_h: f32,
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

/// Cells from the left edge where a header's or row's title starts.
pub const TEXT_COL: f32 = 4.0;
/// Cells reserved on the right of a collapsed header for its summary.
const COLLAPSED_SUMMARY_CELLS: u16 = 4;
/// Cells at the right of the summary row that answer to the sort toggle.
const SORT_TOGGLE_CELLS: f32 = 10.0;
/// Cells at the right of the footer that answer to the mode button.
const MODE_BUTTON_CELLS: f32 = 11.0;
/// Points either side of the separator that grab the resize handle.
const EDGE_TOLERANCE_PT: f32 = 4.0;

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
        let header_h = (cell_h * 1.5).round();
        let row_h = (cell_h * 2.5).round();
        let gap = (cell_h * 0.5).round();
        let footer_h = (cell_h * 1.5).round();
        let list_y = top_h + summary_h;
        let list_h = (height - list_y - footer_h).max(0.0);

        let mut rows = Vec::with_capacity(kinds.len());
        let mut y = 0.0_f32;
        let mut prev_was_pane = false;
        for &kind in kinds {
            let h = match kind {
                SidebarRowKind::Header { .. } => {
                    if prev_was_pane {
                        y += gap;
                    }
                    header_h
                }
                SidebarRowKind::Pane { .. } => row_h,
            };
            rows.push(SidebarRow { kind, y, h });
            y += h;
            prev_was_pane = matches!(kind, SidebarRowKind::Pane { .. });
        }
        if prev_was_pane {
            y += gap;
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
            header_h,
            row_h,
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

    /// Cells available to a title starting at `TEXT_COL`, ending one cell
    /// short of the right edge (`extra_right` more cells for a collapsed
    /// header's summary).
    pub fn title_cells(&self, collapsed_header: bool) -> usize {
        let reserved = 1 + TEXT_COL as u16 + if collapsed_header { COLLAPSED_SUMMARY_CELLS } else { 0 };
        self.width_cells.saturating_sub(reserved) as usize
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
        if py < self.list_y {
            return Some(if px >= self.width - SORT_TOGGLE_CELLS * self.cell_w {
                SidebarHit::SortToggle
            } else {
                SidebarHit::Empty
            });
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
                        if px < self.cell_w * 2.0 {
                            SidebarHit::Chevron(tab_idx)
                        } else {
                            SidebarHit::Header(tab_idx)
                        }
                    }
                    SidebarRowKind::Pane { pane_id, .. } => SidebarHit::Pane(pane_id),
                });
            }
        }
        Some(SidebarHit::Empty)
    }

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

/// The second line of a pane row: what runs in it, then where.
pub fn secondary_line(agent: Option<&str>, process: Option<&str>, cwd_short: &str) -> String {
    match agent.or(process) {
        Some(what) if !cwd_short.is_empty() => format!("{what} \u{b7} {cwd_short}"),
        Some(what) => what.to_string(),
        None => cwd_short.to_string(),
    }
}

/// Copy of the summary row: empty when nothing is waiting or working.
pub fn summary_text(waiting: usize, working: usize) -> String {
    match (waiting, working) {
        (0, 0) => String::new(),
        (w, 0) => format!("{w} waiting"),
        (0, k) => format!("{k} working"),
        (w, k) => format!("{w} waiting \u{b7} {k} working"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: (f32, f32) = (16.0, 32.0);

    fn header(tab_idx: usize, collapsed: bool) -> SidebarRowKind {
        SidebarRowKind::Header { tab_idx, collapsed }
    }

    fn pane(tab_idx: usize, pane_id: PaneId) -> SidebarRowKind {
        SidebarRowKind::Pane { tab_idx, pane_id }
    }

    /// Two groups: tab 0 expanded with two panes, tab 1 collapsed, tab 2
    /// expanded with one pane.
    fn kinds() -> Vec<SidebarRowKind> {
        vec![header(0, false), pane(0, 10), pane(0, 11), header(1, true), header(2, false), pane(2, 30)]
    }

    fn geometry(height: f32, scroll: f32) -> SidebarGeometry {
        SidebarGeometry::new(CELL, 2.0, 28, height, scroll, &kinds(), false)
    }

    #[test]
    fn rows_stack_with_a_gap_after_each_expanded_group_only() {
        let g = geometry(2000.0, 0.0);
        assert_eq!(g.header_h, 48.0);
        assert_eq!(g.row_h, 80.0);
        let ys: Vec<f32> = g.rows.iter().map(|r| r.y).collect();
        // header 0..48, panes 48..128, 128..208, 16px gap, header 224..272
        // (collapsed: no gap), header 272..320, pane 320..400, trailing gap.
        assert_eq!(ys, vec![0.0, 48.0, 128.0, 224.0, 272.0, 320.0]);
        assert_eq!(g.content_h, 416.0);
        assert_eq!(g.width, 28.0 * 16.0);
        assert_eq!(g.sep_w, 2.0);
        assert_eq!(g.list_y, 64.0 + 48.0);
        assert_eq!(g.list_h, 2000.0 - 112.0 - 48.0);
    }

    #[test]
    fn the_hint_takes_a_line_below_the_last_group() {
        let g = SidebarGeometry::new(CELL, 2.0, 28, 2000.0, 0.0, &kinds(), true);
        assert_eq!(g.hint_y, Some(416.0));
        assert_eq!(g.content_h, 416.0 + 48.0);
        assert_eq!(g.hint_screen_y(), Some(112.0 + 416.0));
    }

    #[test]
    fn width_is_snapped_into_range() {
        let g = SidebarGeometry::new(CELL, 2.0, 4, 1000.0, 0.0, &[], false);
        assert_eq!(g.width_cells, 18);
        let g = SidebarGeometry::new(CELL, 2.0, 99, 1000.0, 0.0, &[], false);
        assert_eq!(g.width_cells, 48);
    }

    #[test]
    fn hit_tells_the_regions_apart() {
        let g = geometry(2000.0, 0.0);
        assert_eq!(g.hit(10.0, 10.0), Some(SidebarHit::TopArea));
        // Summary row: sort toggle on the right, nothing on the left.
        assert_eq!(g.hit(10.0, 70.0), Some(SidebarHit::Empty));
        assert_eq!(g.hit(g.width - 20.0, 70.0), Some(SidebarHit::SortToggle));
        // List rows.
        assert_eq!(g.hit(10.0, 112.0 + 10.0), Some(SidebarHit::Chevron(0)));
        assert_eq!(g.hit(100.0, 112.0 + 10.0), Some(SidebarHit::Header(0)));
        assert_eq!(g.hit(100.0, 112.0 + 60.0), Some(SidebarHit::Pane(10)));
        assert_eq!(g.hit(100.0, 112.0 + 130.0), Some(SidebarHit::Pane(11)));
        // The gap between groups is nothing.
        assert_eq!(g.hit(100.0, 112.0 + 210.0), Some(SidebarHit::Empty));
        assert_eq!(g.hit(100.0, 112.0 + 230.0), Some(SidebarHit::Header(1)));
        assert_eq!(g.hit(100.0, 112.0 + 330.0), Some(SidebarHit::Pane(30)));
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
    fn the_resize_handle_wins_near_the_separator() {
        let g = geometry(2000.0, 0.0);
        let edge = g.width + g.sep_w / 2.0;
        assert_eq!(g.hit(edge - 7.0, 500.0), Some(SidebarHit::Edge));
        assert_eq!(g.hit(edge + 7.0, 500.0), Some(SidebarHit::Edge));
        // 4pt at 2x is 8px of tolerance either side; inside it, the row.
        assert_eq!(g.hit(edge - 9.0, 500.0), Some(SidebarHit::Pane(30)));
        assert_eq!(g.hit(edge + 9.0, 500.0), None);
    }

    #[test]
    fn scrolling_shifts_what_a_point_hits() {
        // A 200px list over 416px of rows, scrolled 100px down.
        let g = geometry(112.0 + 200.0 + 48.0, 100.0);
        assert_eq!(g.scroll_y, 100.0);
        assert_eq!(g.hit(100.0, 112.0 + 10.0), Some(SidebarHit::Pane(10)));
        // content y = 112 + 130 - 112 + 100 = 230: header 1.
        assert_eq!(g.hit(100.0, 112.0 + 130.0), Some(SidebarHit::Header(1)));
    }

    #[test]
    fn scroll_is_clamped_to_the_overflow() {
        // A list 200px tall holding 416px of rows can scroll 216px at most.
        let g = geometry(112.0 + 200.0 + 48.0, 999.0);
        assert_eq!(g.max_scroll(), 216.0);
        assert_eq!(g.scroll_y, 216.0);
        assert_eq!(g.overflow(), (true, false));
        let g = geometry(112.0 + 200.0 + 48.0, 0.0);
        assert_eq!(g.overflow(), (false, true));
        // Everything fits: no scroll at all.
        let g = geometry(2000.0, 50.0);
        assert_eq!(g.scroll_y, 0.0);
        assert_eq!(g.overflow(), (false, false));
    }

    #[test]
    fn reveal_moves_the_least_that_shows_the_whole_row() {
        let g = geometry(112.0 + 200.0 + 48.0, 0.0);
        // Row 2 (pane 11, 128..208) sticks out below a 200px list.
        assert_eq!(g.reveal(2), 8.0);
        // Row 0 is already visible.
        assert_eq!(g.reveal(0), 0.0);
        let g = geometry(112.0 + 200.0 + 48.0, 150.0);
        // Row 0 (0..48) is above: scroll back to its top.
        assert_eq!(g.reveal(0), 0.0);
        // Row 5 (320..400) sits inside 150..350? No: bottom 400 > 350.
        assert_eq!(g.reveal(5), 200.0);
        assert_eq!(g.row_for_pane(30), Some(5));
        assert_eq!(g.row_for_pane(99), None);
        assert_eq!(g.row_for_header(1), Some(3));
    }

    #[test]
    fn insertion_index_follows_the_midpoint_rule() {
        let g = geometry(2000.0, 0.0);
        // Above the centre of header 0 (24): before it.
        assert_eq!(g.insertion_index(112.0 + 10.0), 0);
        // Below its centre: before header 1.
        assert_eq!(g.insertion_index(112.0 + 100.0), 1);
        // Header 1 spans 224..272, centre 248.
        assert_eq!(g.insertion_index(112.0 + 240.0), 1);
        assert_eq!(g.insertion_index(112.0 + 260.0), 2);
        // Past the last header's centre (296): at the end.
        assert_eq!(g.insertion_index(112.0 + 300.0), 3);
        assert_eq!(g.insertion_index(1500.0), 3);
        assert_eq!(g.insertion_line_y(0), 112.0);
        assert_eq!(g.insertion_line_y(1), 112.0 + 224.0);
        assert_eq!(g.insertion_line_y(3), 112.0 + 400.0);
    }

    #[test]
    fn autoscroll_zones_hug_the_list_edges() {
        let g = geometry(2000.0, 0.0);
        assert_eq!(g.autoscroll_direction(g.list_y + 10.0), -1);
        assert_eq!(g.autoscroll_direction(g.list_y + 100.0), 0);
        assert_eq!(g.autoscroll_direction(g.list_y + g.list_h - 10.0), 1);
    }

    #[test]
    fn title_cells_leave_room_for_the_margin_and_the_summary() {
        let g = geometry(2000.0, 0.0);
        assert_eq!(g.title_cells(false), 28 - 5);
        assert_eq!(g.title_cells(true), 28 - 9);
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
    fn summary_text_only_mentions_what_is_there() {
        assert_eq!(summary_text(0, 0), "");
        assert_eq!(summary_text(1, 0), "1 waiting");
        assert_eq!(summary_text(0, 2), "2 working");
        assert_eq!(summary_text(1, 2), "1 waiting \u{b7} 2 working");
    }

    #[test]
    fn state_square_follows_the_priority_order() {
        use StateSquare::*;
        assert_eq!(StateSquare::from_flags(true, true, true, true, true, false), Awaiting);
        assert_eq!(StateSquare::from_flags(false, true, true, true, true, false), Bell);
        assert_eq!(StateSquare::from_flags(false, false, true, true, true, false), Completion);
        // The pane being looked at never shows a bell or a completion.
        assert_eq!(StateSquare::from_flags(false, true, true, true, true, true), Working);
        assert_eq!(StateSquare::from_flags(false, false, false, false, true, false), IdleAgent);
        assert_eq!(StateSquare::from_flags(false, false, false, false, false, false), None);
        assert!(IdleAgent.hollow());
        assert!(!Working.hollow());
        assert_eq!(None.color(), Option::None);
    }

    #[test]
    fn a_collapsed_group_summarises_waiting_over_working_over_nothing() {
        use StateSquare::*;
        assert_eq!(StateSquare::group_summary([Working, Awaiting, None].into_iter()), Awaiting);
        assert_eq!(StateSquare::group_summary([Bell, Working].into_iter()), Working);
        assert_eq!(StateSquare::group_summary([Bell, IdleAgent].into_iter()), None);
        assert_eq!(StateSquare::group_summary(std::iter::empty()), None);
    }

    #[test]
    fn activity_sort_puts_urgent_tabs_first_and_keeps_ties_in_tab_order() {
        use StateSquare::*;
        let states = [None, Working, Awaiting, Working, IdleAgent];
        assert_eq!(display_order(SidebarSort::Kova, &states), vec![0, 1, 2, 3, 4]);
        assert_eq!(display_order(SidebarSort::Activity, &states), vec![2, 1, 3, 4, 0]);
        assert_eq!(SidebarSort::Kova.toggled(), SidebarSort::Activity);
        assert_eq!(SidebarSort::Activity.label(), "activity");
    }
}
