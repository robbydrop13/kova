//! The sidebar inside a window: its transient state (scroll, hover, press,
//! drags, the caught-up flash), the mouse handlers, the tile and header
//! actions with their context menus, the mode switch, and the per-frame data
//! the renderer paints from. All positions come from
//! `sidebar::SidebarGeometry`, so a click lands on the tile that was drawn.
//! The pure parts live in `sidebar.rs`.

use super::*;
use super::sidebar::{
    self, display_order, drop_index, edit_tail, format_age, secondary_line, short_cwd, summary_runs,
    swap_chain, truncate_path, wrap_text, CollapsedSummary, NextPill, PaneFlags, SidebarGeometry,
    SidebarHit, SidebarRowKind, SidebarSort, SummaryRun, TileButton, TileLayout, TileState, AGING_SECS,
};
use crate::config::LayoutMode;
use crate::prompt_preview::PromptPreview;
use crate::renderer::{SidebarHeaderRender, SidebarRowBg, SidebarRowRender, SidebarTileRender};

/// Pixels of vertical travel before a pressed header or tile lifts into a drag.
const DRAG_THRESHOLD: f32 = 3.0;
/// How often a drag parked near a list edge scrolls one header height.
const AUTOSCROLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
/// How long `✓ All caught up` stays up once the last unread was read.
const CAUGHT_UP_SECS: f32 = 1.6;

/// A header being dragged to another position in the tab order.
#[derive(Clone, Copy)]
struct SidebarDrag {
    tab_idx: usize,
    start_y: f32,
    current_y: f32,
    /// Where inside the header the cursor grabbed it, so the floating copy
    /// keeps that offset.
    grab_offset: f32,
    dragging: bool,
    last_autoscroll: std::time::Instant,
}

/// A tile being dragged to another slot of its column.
#[derive(Clone, Copy)]
struct PaneDrag {
    pane_id: PaneId,
    start_y: f32,
    current_y: f32,
    grab_offset: f32,
    dragging: bool,
}

/// What a tile's context menu can do, dispatched by the item's tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaneAction {
    Open,
    Stop,
    StartClaude,
    Rename,
    ToggleBookmark,
    Minimize,
    Restore,
    Close,
}

impl PaneAction {
    const ALL: [PaneAction; 8] = [
        PaneAction::Open,
        PaneAction::Stop,
        PaneAction::StartClaude,
        PaneAction::Rename,
        PaneAction::ToggleBookmark,
        PaneAction::Minimize,
        PaneAction::Restore,
        PaneAction::Close,
    ];

    fn from_tag(tag: isize) -> Option<Self> {
        usize::try_from(tag).ok().and_then(|i| Self::ALL.get(i).copied())
    }

    fn tag(self) -> isize {
        Self::ALL.iter().position(|a| *a == self).unwrap_or(0) as isize
    }
}

/// What a header's context menu can do, after the colour items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TabAction {
    Color(usize),
    NoColor,
    Rename,
    AddPane,
    CollapseOthers,
    Close,
}

impl TabAction {
    fn from_tag(tag: isize) -> Option<Self> {
        match tag {
            0..=5 => Some(TabAction::Color(tag as usize)),
            6 => Some(TabAction::NoColor),
            10 => Some(TabAction::Rename),
            11 => Some(TabAction::AddPane),
            12 => Some(TabAction::CollapseOthers),
            13 => Some(TabAction::Close),
            _ => None,
        }
    }

    fn tag(self) -> isize {
        match self {
            TabAction::Color(i) => i as isize,
            TabAction::NoColor => 6,
            TabAction::Rename => 10,
            TabAction::AddPane => 11,
            TabAction::CollapseOthers => 12,
            TabAction::Close => 13,
        }
    }
}

/// A menu row: a label and its tag, or a separator.
enum MenuRow {
    Item(String, isize),
    Separator,
}

pub(super) struct SidebarState {
    scroll_y: f32,
    sort: SidebarSort,
    hovered: Option<SidebarHit>,
    /// What the mouse went down on; the action fires on mouse up inside the
    /// same target, like a button.
    pressed: Option<SidebarHit>,
    drag: Option<SidebarDrag>,
    pane_drag: Option<PaneDrag>,
    edge_drag: bool,
    /// The mouse went down inside the sidebar: the matching mouse up is ours
    /// whatever it lands on.
    mouse_down: bool,
    /// (active tab, focused pane) the list last scrolled to show, so a focus
    /// change reveals its row exactly once and a manual scroll then sticks.
    last_reveal: Option<(usize, PaneId)>,
    /// Unread count of the previous frame, to catch it dropping to zero.
    last_unread: Option<usize>,
    /// Frames left of the `✓ All caught up` flash.
    caught_up_frames: u32,
    /// The pane a context menu is open for.
    menu_pane: PaneId,
    /// The tab a context menu is open for.
    menu_tab: usize,
}

impl SidebarState {
    pub(super) fn new() -> Self {
        SidebarState {
            scroll_y: 0.0,
            sort: SidebarSort::Kova,
            hovered: None,
            pressed: None,
            drag: None,
            pane_drag: None,
            edge_drag: false,
            mouse_down: false,
            last_reveal: None,
            last_unread: None,
            caught_up_frames: 0,
            menu_pane: 0,
            menu_tab: 0,
        }
    }
}

/// Everything the renderer needs for one frame of the sidebar. Owned, so the
/// tick can build it before taking the renderer lock.
pub(super) struct SidebarFrame {
    pub(super) geometry: SidebarGeometry,
    pub(super) rows: Vec<SidebarRowRender>,
    pub(super) summary: Vec<(String, SummaryRun)>,
    pub(super) sort: SidebarSort,
    pub(super) pill: NextPill,
    pub(super) hovered: Option<SidebarHit>,
    pub(super) pressed: Option<SidebarHit>,
    pub(super) insertion_y: Option<f32>,
    pub(super) pane_insertion_y: Option<f32>,
    pub(super) lifted: Option<(usize, f32)>,
}

impl KovaView {
    /// Width the sidebar and its separator take from the panes, in pixels.
    /// Zero in tabs mode, and zero when the window is too narrow to keep a
    /// split-wide column next to it: the window then falls back to the tab
    /// bar until a resize makes room again, without touching the setting.
    pub(super) fn sidebar_total_width(&self) -> f32 {
        if sidebar::layout_mode() != LayoutMode::Sidebar {
            return 0.0;
        }
        let Some(renderer) = self.ivars().renderer.get() else { return 0.0 };
        let cell_w = renderer.read().cell_size().0;
        let scale = self.backing_scale();
        let width = (sidebar::width_cells() as f32 * cell_w).round();
        let total = width + scale.round().max(1.0);
        let full_w = self.drawable_viewport().width;
        if full_w - total < self.min_split_width_px() {
            return 0.0;
        }
        total
    }

    pub(super) fn sidebar_active(&self) -> bool {
        self.sidebar_total_width() > 0.0
    }

    pub(super) fn sidebar_sort(&self) -> SidebarSort {
        self.ivars().sidebar.borrow().sort
    }

    pub fn set_sidebar_sort(&self, sort: SidebarSort) {
        self.ivars().sidebar.borrow_mut().sort = sort;
    }

    /// Switch the whole app between the tab bar and the sidebar, remember
    /// it, and relayout every window.
    pub(super) fn set_layout_mode(&self, mode: LayoutMode) {
        if sidebar::layout_mode() == mode {
            return;
        }
        sidebar::set_layout_mode(mode);
        {
            let mut st = self.ivars().sidebar.borrow_mut();
            st.hovered = None;
            st.pressed = None;
            st.drag = None;
            st.pane_drag = None;
            st.edge_drag = false;
        }
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let ad = crate::app::app_delegate(mtm);
        let windows = ad.ivars().windows.borrow();
        for win in windows.iter() {
            if let Some(view) = crate::app::kova_view(win) {
                view.layout_changed();
            }
        }
    }

    pub(super) fn toggle_layout_mode(&self) {
        let next = match sidebar::layout_mode() {
            LayoutMode::Tabs => LayoutMode::Sidebar,
            LayoutMode::Sidebar => LayoutMode::Tabs,
        };
        self.set_layout_mode(next);
    }

    /// The pane area moved or changed width: keep the active tab's scroll in
    /// range and hand every pane its new size.
    fn layout_changed(&self) {
        {
            let content_w = self.content_viewport().width;
            let min_w = self.min_split_width_px();
            let mut tabs = self.ivars().tabs.borrow_mut();
            let idx = self.ivars().active_tab.get();
            if let Some(tab) = tabs.get_mut(idx) {
                tab.clamp_scroll(content_w, min_w);
                tab.mark_all_dirty();
            }
        }
        self.resize_all_panes();
        self.mark_dirty();
    }

    /// The row list for this window: one header per tab in display order,
    /// the tiles of every unfolded tab under theirs, column by column. Also
    /// returns the total pane count.
    fn sidebar_row_kinds(&self, tabs: &[Tab], sort: SidebarSort) -> (Vec<SidebarRowKind>, usize) {
        let active_idx = self.ivars().active_tab.get();
        let content_cells = self.sidebar_content_cells();
        let tab_states: Vec<TileState> = tabs
            .iter()
            .enumerate()
            .map(|(ti, tab)| {
                let mut states = Vec::new();
                tab.for_each_pane(&mut |pane| {
                    let seen = ti == active_idx && pane.id == tab.focused_pane;
                    states.push(pane_state(pane, seen));
                });
                TileState::most_urgent(states.into_iter())
            })
            .collect();
        let mut kinds = Vec::new();
        let mut total_panes = 0;
        for ti in display_order(sort, &tab_states) {
            let tab = &tabs[ti];
            kinds.push(SidebarRowKind::Header { tab_idx: ti, collapsed: tab.collapsed });
            for (column, col) in tab.columns.iter().enumerate() {
                for pane in &col.panes {
                    total_panes += 1;
                    if !tab.collapsed {
                        let seen = ti == active_idx && pane.id == tab.focused_pane;
                        let tile = pane_tile(pane, seen, content_cells);
                        kinds.push(SidebarRowKind::Pane { tab_idx: ti, pane_id: pane.id, column, tile });
                    }
                }
            }
        }
        (kinds, total_panes)
    }

    /// Cells a tile's text line holds at the current width.
    fn sidebar_content_cells(&self) -> usize {
        // content_x = 2.5 cw, content_right = W - 2 cw: W - 4.5 cells.
        (sidebar::width_cells() as f32 - 4.5).floor().max(0.0) as usize
    }

    /// The geometry of the sidebar as it is drawn right now, `None` when the
    /// window shows the tab bar.
    pub(super) fn sidebar_geometry(&self) -> Option<SidebarGeometry> {
        if !self.sidebar_active() {
            return None;
        }
        let renderer = self.ivars().renderer.get()?;
        let cell = renderer.read().cell_size();
        let height = self.drawable_viewport().height - self.global_bar_height();
        let (scroll_y, sort) = {
            let st = self.ivars().sidebar.borrow();
            (st.scroll_y, st.sort)
        };
        let tabs = self.ivars().tabs.borrow();
        let (kinds, total_panes) = self.sidebar_row_kinds(&tabs, sort);
        Some(SidebarGeometry::new(
            cell,
            self.backing_scale(),
            sidebar::width_cells(),
            height,
            scroll_y,
            &kinds,
            total_panes < 3,
        ))
    }

    /// Build the frame's sidebar data. Runs once per tick; also where the
    /// list follows a focus change, a drag near an edge keeps scrolling, and
    /// the caught-up flash counts down.
    pub(super) fn sidebar_frame(&self) -> Option<SidebarFrame> {
        let mut g = self.sidebar_geometry()?;
        // Cmd+J's tiers across every window, for the Next pill.
        let attention = self.collect_attention();
        let tabs = self.ivars().tabs.borrow();
        let active_idx = self.ivars().active_tab.get();
        let mut st = self.ivars().sidebar.borrow_mut();

        // Follow the focus: reveal the focused pane's tile (its header when
        // the group is folded), once per focus change.
        if let Some(tab) = tabs.get(active_idx) {
            let key = (active_idx, tab.focused_pane);
            if st.last_reveal != Some(key) {
                st.last_reveal = Some(key);
                let row = if tab.collapsed {
                    g.row_for_header(active_idx)
                } else {
                    g.row_for_pane(tab.focused_pane)
                };
                if let Some(row) = row {
                    g.scroll_y = g.reveal(row);
                }
            }
        }
        // A tab drag parked near the top or bottom of the list keeps scrolling.
        if let Some(d) = st.drag.as_mut().filter(|d| d.dragging) {
            let dir = g.autoscroll_direction(d.current_y);
            if dir != 0 && d.last_autoscroll.elapsed() >= AUTOSCROLL_INTERVAL {
                d.last_autoscroll = std::time::Instant::now();
                g.scroll_y = g.clamp_scroll(g.scroll_y + dir as f32 * g.header_h);
                self.mark_dirty();
            }
        }
        st.scroll_y = g.scroll_y;

        // The Next pill: unread first, then idle; a green flash the moment
        // the last unread was read.
        let unread = attention.unread.len();
        let idle = attention.idle_agent.len();
        if st.last_unread.is_some_and(|before| before > 0) && unread == 0 {
            let fps = self.ivars().config.get().map(|c| c.terminal.fps).unwrap_or(60) as f32;
            st.caught_up_frames = (fps * CAUGHT_UP_SECS) as u32;
        }
        st.last_unread = Some(unread);
        if st.caught_up_frames > 0 {
            st.caught_up_frames -= 1;
            self.mark_dirty();
        }
        let pill = NextPill::of(unread, idle, st.caught_up_frames > 0);

        let home = std::env::var("HOME").unwrap_or_default();
        let bookmark_keys = self.ivars().bookmark_keys.borrow();
        let rename_tab = self.ivars().rename_tab.borrow();
        let rename_pane = self.ivars().rename_pane.borrow();
        let edit_buffer = |input: &str, cursor: usize| {
            let before: String = input.chars().take(cursor).collect();
            let after: String = input.chars().skip(cursor).collect();
            format!("{before}\u{258f}{after}")
        };
        let now = now_epoch_secs();

        // Summary counts for this window.
        let mut waiting = 0usize;
        let mut working = 0usize;
        let mut idle_here = 0usize;
        for (ti, tab) in tabs.iter().enumerate() {
            tab.for_each_pane(&mut |pane| {
                let seen = ti == active_idx && pane.id == tab.focused_pane;
                match pane_state(pane, seen) {
                    TileState::Awaiting => waiting += 1,
                    TileState::Working | TileState::Starting => working += 1,
                    TileState::Idle => idle_here += 1,
                    _ => {}
                }
            });
        }

        let row_bg = |hits: &[SidebarHit]| -> SidebarRowBg {
            let is = |h: Option<SidebarHit>| h.is_some_and(|h| hits.contains(&h));
            if is(st.pressed) {
                SidebarRowBg::Pressed
            } else if is(st.hovered) {
                SidebarRowBg::Hover
            } else {
                SidebarRowBg::Plain
            }
        };
        let content_cells = g.content_cells();
        let mut rows = Vec::with_capacity(g.rows.len());
        for row in &g.rows {
            match row.kind {
                SidebarRowKind::Header { tab_idx, collapsed } => {
                    let tab = &tabs[tab_idx];
                    let is_active = tab_idx == active_idx;
                    let renaming = is_active && rename_tab.is_some();
                    let title = if let (true, Some(rs)) = (renaming, rename_tab.as_ref()) {
                        edit_tail(&edit_buffer(&rs.input, rs.cursor), g.title_cells(collapsed))
                    } else {
                        tab.title()
                    };
                    let mut states = Vec::new();
                    tab.for_each_pane(&mut |pane| {
                        states.push(pane_state(pane, is_active && pane.id == tab.focused_pane));
                    });
                    rows.push(SidebarRowRender::Header(SidebarHeaderRender {
                        title,
                        number: tab_idx + 1,
                        color: tab.color,
                        active_tab: is_active,
                        collapsed,
                        summary: CollapsedSummary::of(states.into_iter()),
                        bg: row_bg(&[SidebarHit::Header(tab_idx), SidebarHit::Chevron(tab_idx)]),
                        add_hovered: st.hovered == Some(SidebarHit::HeaderAdd(tab_idx)),
                        renaming,
                    }));
                }
                SidebarRowKind::Pane { tab_idx, pane_id, tile, .. } => {
                    let tab = &tabs[tab_idx];
                    let Some(pane) = tab.pane(pane_id) else { continue };
                    let is_active = tab_idx == active_idx;
                    let focused = is_active && pane.id == tab.focused_pane;
                    let renaming = focused && rename_pane.is_some();
                    let title = if let (true, Some(rs)) = (renaming, rename_pane.as_ref()) {
                        edit_tail(&edit_buffer(&rs.input, rs.cursor), g.title_cells_beside(9))
                    } else {
                        pane.display_title("shell")
                    };
                    let (cwd, bookmarked) = {
                        let term = pane.terminal.read();
                        let cwd = term.cwd.clone().unwrap_or_default();
                        let bookmarked = match pane.agent_session.borrow().as_ref() {
                            Some(session) => bookmark_keys.contains(&session.id),
                            None => !cwd.is_empty() && bookmark_keys.contains(&cwd),
                        };
                        (cwd, bookmarked)
                    };
                    let agent = pane.agent_kind().map(|a| a.as_str());
                    let process = pane.fg_process().map(|p| p.name);
                    let what_len = agent.map(str::len).or(process.as_ref().map(|p| p.chars().count()));
                    let line_cells = if tile.bare_shell {
                        content_cells.saturating_sub(sidebar::START_CLAUDE_LABEL.chars().count() + 1)
                    } else {
                        content_cells
                    };
                    let cwd_cells = line_cells.saturating_sub(what_len.map_or(0, |n| n + 3));
                    let cwd_short = truncate_path(&short_cwd(&cwd, &home), cwd_cells);
                    let (extra, question_lines, age) = tile_extra_lines(pane, &tile, content_cells, now);
                    let hovered = st.hovered.and_then(SidebarHit::pane) == Some(pane_id);
                    let hovered_button = match st.hovered {
                        Some(SidebarHit::PaneButton(id, b)) if id == pane_id => Some(b),
                        _ => None,
                    };
                    let pressed_button = match st.pressed {
                        Some(SidebarHit::PaneButton(id, b)) if id == pane_id => Some(b),
                        _ => None,
                    };
                    rows.push(SidebarRowRender::Tile(SidebarTileRender {
                        tile,
                        title,
                        secondary: secondary_line(agent, process.as_deref(), &cwd_short),
                        extra,
                        question_lines,
                        age,
                        focused,
                        bookmarked,
                        renaming,
                        bg: row_bg(&[SidebarHit::Pane(pane_id)]),
                        hovered,
                        hovered_button,
                        pressed_button,
                    }));
                }
            }
        }

        let (insertion_y, mut lifted) = match st.drag {
            Some(d) if d.dragging => {
                let k = g.insertion_index(d.current_y);
                (
                    Some(g.insertion_line_y(k)),
                    g.row_for_header(d.tab_idx).map(|r| (r, d.current_y - d.grab_offset)),
                )
            }
            _ => (None, None),
        };
        let mut pane_insertion_y = None;
        if let Some(d) = st.pane_drag.filter(|d| d.dragging) {
            if let Some(idx) = g.row_for_pane(d.pane_id) {
                let run = g.pane_run(idx);
                if let Some(slot) = g.pane_insertion_slot(&run, d.current_y) {
                    pane_insertion_y = Some(g.pane_insertion_line_y(&run, slot));
                }
                lifted = Some((idx, d.current_y - d.grab_offset));
            }
        }

        Some(SidebarFrame {
            summary: summary_runs(waiting, working, idle_here),
            sort: st.sort,
            pill,
            hovered: st.hovered,
            pressed: st.pressed,
            insertion_y,
            pane_insertion_y,
            lifted,
            geometry: g,
            rows,
        })
    }

    // ---------------------------------------------------------------
    // Mouse
    // ---------------------------------------------------------------

    /// Mouse down in the sidebar. Returns false when the point is not ours.
    pub(super) fn hit_test_sidebar(&self, px: f32, py: f32, event: &NSEvent) -> bool {
        let Some(g) = self.sidebar_geometry() else { return false };
        let Some(hit) = g.hit(px, py) else { return false };
        {
            let mut st = self.ivars().sidebar.borrow_mut();
            st.pressed = None;
            st.drag = None;
            st.pane_drag = None;
            st.edge_drag = false;
            st.mouse_down = true;
        }
        match hit {
            SidebarHit::Edge => self.ivars().sidebar.borrow_mut().edge_drag = true,
            SidebarHit::TopArea => {
                if let Some(win) = self.window() {
                    if event.clickCount() == 2 {
                        win.zoom(None);
                    } else {
                        win.performWindowDragWithEvent(event);
                    }
                }
            }
            SidebarHit::Header(tab_idx) => {
                if event.clickCount() == 2 {
                    self.do_switch_tab(tab_idx);
                    self.start_rename_tab();
                } else {
                    let row_y = g.row_for_header(tab_idx).map_or(py, |r| g.row_screen_y(r));
                    let mut st = self.ivars().sidebar.borrow_mut();
                    st.pressed = Some(hit);
                    // Reordering only means something in tab order.
                    if st.sort == SidebarSort::Kova {
                        st.drag = Some(SidebarDrag {
                            tab_idx,
                            start_y: py,
                            current_y: py,
                            grab_offset: py - row_y,
                            dragging: false,
                            last_autoscroll: std::time::Instant::now(),
                        });
                    }
                }
            }
            SidebarHit::Pane(pane_id) => {
                if event.clickCount() == 2 {
                    self.focus_pane_in_window(pane_id);
                    self.start_rename_pane();
                } else {
                    let row_y = g.row_for_pane(pane_id).map_or(py, |r| g.row_screen_y(r));
                    let mut st = self.ivars().sidebar.borrow_mut();
                    st.pressed = Some(hit);
                    st.pane_drag = Some(PaneDrag {
                        pane_id,
                        start_y: py,
                        current_y: py,
                        grab_offset: py - row_y,
                        dragging: false,
                    });
                }
            }
            SidebarHit::Chevron(_)
            | SidebarHit::HeaderAdd(_)
            | SidebarHit::PaneButton(..)
            | SidebarHit::SortToggle
            | SidebarHit::NextPill
            | SidebarHit::ModeButton => {
                self.ivars().sidebar.borrow_mut().pressed = Some(hit);
            }
            SidebarHit::Empty => {}
        }
        self.mark_dirty();
        true
    }

    /// Mouse drag that started in the sidebar: edge resize, header or tile
    /// drag, or a press that leaves its target (which cancels the click).
    pub(super) fn sidebar_mouse_dragged(&self, event: &NSEvent) -> bool {
        if !self.ivars().sidebar.borrow().mouse_down {
            return false;
        }
        let (px, py) = self.event_to_pixel(event);
        if self.ivars().sidebar.borrow().edge_drag {
            let Some(renderer) = self.ivars().renderer.get() else { return true };
            let cell_w = renderer.read().cell_size().0;
            let cells = (px / cell_w).round().max(0.0) as u16;
            if sidebar::set_width_cells(cells) {
                self.layout_changed();
            }
            return true;
        }
        let drag = self.ivars().sidebar.borrow().drag;
        if let Some(mut d) = drag {
            d.current_y = py;
            if !d.dragging && (py - d.start_y).abs() >= DRAG_THRESHOLD {
                d.dragging = true;
            }
            let mut st = self.ivars().sidebar.borrow_mut();
            if d.dragging {
                st.pressed = None;
            }
            st.drag = Some(d);
            drop(st);
            self.mark_dirty();
            return true;
        }
        let pane_drag = self.ivars().sidebar.borrow().pane_drag;
        if let Some(mut d) = pane_drag {
            d.current_y = py;
            if !d.dragging && (py - d.start_y).abs() >= DRAG_THRESHOLD {
                d.dragging = true;
            }
            let mut st = self.ivars().sidebar.borrow_mut();
            if d.dragging {
                st.pressed = None;
            }
            st.pane_drag = Some(d);
            drop(st);
            self.mark_dirty();
            return true;
        }
        let pressed = self.ivars().sidebar.borrow().pressed;
        if pressed.is_some() {
            let hit = self.sidebar_geometry().and_then(|g| g.hit(px, py));
            if hit != pressed {
                self.ivars().sidebar.borrow_mut().pressed = None;
                self.mark_dirty();
            }
        }
        true
    }

    /// Mouse up after a sidebar mouse down: fire the click, drop the tab or
    /// the tile, or remember the new width.
    pub(super) fn sidebar_mouse_up(&self, event: &NSEvent) -> bool {
        let (edge, drag, pane_drag, pressed) = {
            let mut st = self.ivars().sidebar.borrow_mut();
            if !st.mouse_down {
                return false;
            }
            st.mouse_down = false;
            (
                std::mem::take(&mut st.edge_drag),
                st.drag.take(),
                st.pane_drag.take(),
                st.pressed.take(),
            )
        };
        let (px, py) = self.event_to_pixel(event);
        if edge {
            sidebar::persist();
            return true;
        }
        if let Some(d) = drag.filter(|d| d.dragging) {
            if let Some(g) = self.sidebar_geometry() {
                self.reorder_tab(d.tab_idx, g.insertion_index(py));
            }
            self.mark_dirty();
            return true;
        }
        if let Some(d) = pane_drag.filter(|d| d.dragging) {
            if let Some(g) = self.sidebar_geometry() {
                self.drop_pane(&g, d.pane_id, py);
            }
            self.mark_dirty();
            return true;
        }
        if let Some(target) = pressed {
            let hit = self.sidebar_geometry().and_then(|g| g.hit(px, py));
            if hit == Some(target) {
                self.sidebar_activate(target);
            }
        }
        self.mark_dirty();
        true
    }

    /// What a completed click does.
    fn sidebar_activate(&self, hit: SidebarHit) {
        let active_idx = self.ivars().active_tab.get();
        match hit {
            SidebarHit::Chevron(tab_idx) => self.toggle_tab_collapsed(tab_idx),
            SidebarHit::Header(tab_idx) => {
                if tab_idx == active_idx {
                    self.toggle_tab_collapsed(tab_idx);
                } else {
                    self.do_switch_tab(tab_idx);
                }
            }
            SidebarHit::HeaderAdd(tab_idx) => self.tab_action(tab_idx, TabAction::AddPane),
            SidebarHit::Pane(pane_id) => {
                self.focus_pane_in_window(pane_id);
            }
            SidebarHit::PaneButton(pane_id, button) => {
                let action = match button {
                    TileButton::Close => PaneAction::Close,
                    TileButton::Minimize => PaneAction::Minimize,
                    TileButton::Restore => PaneAction::Restore,
                    TileButton::Stop => PaneAction::Stop,
                    TileButton::StartClaude => PaneAction::StartClaude,
                    TileButton::Open => PaneAction::Open,
                };
                self.dispatch_pane_action(pane_id, action);
            }
            SidebarHit::SortToggle => {
                let mut st = self.ivars().sidebar.borrow_mut();
                st.sort = st.sort.toggled();
            }
            SidebarHit::NextPill => {
                // `Nothing to read` is not a button: only a pill with a
                // badge jumps.
                let sets = self.collect_attention();
                if NextPill::of(sets.unread.len(), sets.idle_agent.len(), false).clickable() {
                    self.do_focus_next_attention();
                }
            }
            SidebarHit::ModeButton => self.set_layout_mode(LayoutMode::Tabs),
            SidebarHit::Edge | SidebarHit::TopArea | SidebarHit::Empty => {}
        }
    }

    fn toggle_tab_collapsed(&self, tab_idx: usize) {
        let mut tabs = self.ivars().tabs.borrow_mut();
        if let Some(tab) = tabs.get_mut(tab_idx) {
            tab.collapsed = !tab.collapsed;
        }
    }

    /// Drop a dragged header at insertion position `k` (before the k-th
    /// group, or at the end).
    fn reorder_tab(&self, from: usize, k: usize) {
        let to = if k > from { k - 1 } else { k };
        let tab_id = {
            let tabs = self.ivars().tabs.borrow();
            if to == from || from >= tabs.len() {
                return;
            }
            tabs[from].id
        };
        self.ipc_move_tab(tab_id, to);
    }

    /// Drop a dragged tile at the slot under `py` in its column: replay the
    /// move as adjacent swaps through `Tab::swap_panes`, the primitive behind
    /// the `swap-pane` IPC command. Outside its run, the tile snaps back.
    fn drop_pane(&self, g: &SidebarGeometry, pane_id: PaneId, py: f32) {
        let Some(idx) = g.row_for_pane(pane_id) else { return };
        let run = g.pane_run(idx);
        let Some(slot) = g.pane_insertion_slot(&run, py) else { return };
        let (tab_idx, ids): (usize, Vec<PaneId>) = {
            let mut tab_idx = 0;
            let ids = g.rows[run.clone()]
                .iter()
                .filter_map(|r| match r.kind {
                    SidebarRowKind::Pane { tab_idx: t, pane_id, .. } => {
                        tab_idx = t;
                        Some(pane_id)
                    }
                    _ => None,
                })
                .collect();
            (tab_idx, ids)
        };
        let from = idx - run.start;
        let to = drop_index(from, slot);
        let chain = swap_chain(ids.len(), from, to);
        if chain.is_empty() {
            return;
        }
        let mut tabs = self.ivars().tabs.borrow_mut();
        let Some(tab) = tabs.get_mut(tab_idx) else { return };
        for (a, b) in chain {
            tab.swap_panes(ids[a], ids[b], crate::pane::NavDirection::Down);
        }
        tab.mark_all_dirty();
        drop(tabs);
        self.resize_all_panes();
    }

    /// Hover tracking and cursor while the mouse moves over the sidebar.
    /// Returns false when the point is not ours.
    pub(super) fn sidebar_mouse_moved(&self, event: &NSEvent) -> bool {
        let (px, py) = self.event_to_pixel(event);
        let hit = self.sidebar_geometry().and_then(|g| g.hit(px, py));
        let hovered = match hit {
            Some(SidebarHit::Edge) | Some(SidebarHit::TopArea) | Some(SidebarHit::Empty) | None => None,
            Some(h) => Some(h),
        };
        let changed = {
            let mut st = self.ivars().sidebar.borrow_mut();
            let changed = st.hovered != hovered;
            st.hovered = hovered;
            changed
        };
        if changed {
            self.mark_dirty();
        }
        match hit {
            None => false,
            Some(SidebarHit::Edge) => {
                #[allow(deprecated)]
                NSCursor::resizeLeftRightCursor().set();
                true
            }
            Some(h) => {
                NSCursor::arrowCursor().set();
                // The glyph buttons carry tooltips; nothing else in the
                // sidebar does, and a pane status bar scrolled under it must
                // not keep its own up.
                if let Some(renderer) = self.ivars().renderer.get() {
                    let tooltip = match h {
                        SidebarHit::PaneButton(..) => renderer.read().hit_test_tooltip(px, py),
                        _ => None,
                    };
                    let mut r = renderer.write();
                    if r.active_tooltip != tooltip {
                        r.active_tooltip = tooltip;
                        drop(r);
                        self.mark_dirty();
                    }
                }
                true
            }
        }
    }

    /// Wheel over the sidebar scrolls the list; the horizontal component is
    /// dropped, never forwarded to the panes.
    pub(super) fn sidebar_scroll(&self, event: &NSEvent, is_trackpad: bool) -> bool {
        let (px, py) = self.event_to_pixel(event);
        let Some(g) = self.sidebar_geometry() else { return false };
        if px >= g.total_w() || py < 0.0 || py >= g.height {
            return false;
        }
        let dy = event.scrollingDeltaY() as f32;
        let delta = if is_trackpad {
            let sensitivity = self.ivars().config.get()
                .map(|c| c.terminal.scroll_sensitivity)
                .unwrap_or(TerminalConfig::default().scroll_sensitivity) as f32;
            dy * g.scale * (sensitivity / 6.0)
        } else {
            dy * g.cell_h
        };
        let mut st = self.ivars().sidebar.borrow_mut();
        st.scroll_y = g.clamp_scroll(st.scroll_y - delta);
        drop(st);
        self.mark_dirty();
        true
    }

    /// Right click: the tile menu on a pane, the header menu on a group.
    /// Returns false when the point is not ours.
    pub(super) fn sidebar_right_click(&self, px: f32, py: f32, event: &NSEvent) -> bool {
        let Some(g) = self.sidebar_geometry() else { return false };
        let Some(hit) = g.hit(px, py) else { return false };
        if let Some(pane_id) = hit.pane() {
            self.show_sidebar_pane_menu(event, pane_id);
        } else if let Some(tab_idx) = hit.header() {
            self.show_sidebar_tab_menu(event, tab_idx);
        }
        true
    }

    // ---------------------------------------------------------------
    // Actions and menus
    // ---------------------------------------------------------------

    /// The pane `id` lives in, with the reads the menus and actions need.
    fn pane_snapshot(&self, pane_id: PaneId) -> Option<(String, bool, bool, bool, bool, bool)> {
        let tabs = self.ivars().tabs.borrow();
        let bookmark_keys = self.ivars().bookmark_keys.borrow();
        for tab in tabs.iter() {
            if let Some(pane) = tab.pane(pane_id) {
                let bookmarked = match pane.agent_session_id() {
                    Some(id) => bookmark_keys.contains(&id),
                    None => pane.cwd().is_some_and(|cwd| bookmark_keys.contains(&cwd)),
                };
                return Some((
                    pane.display_title("shell"),
                    pane.is_working(),
                    pane.has_permission_prompt(),
                    pane.is_bare_shell(),
                    pane.minimized,
                    bookmarked,
                ));
            }
        }
        None
    }

    /// The tile's context menu, KovaLink's swipes and sheets as items.
    fn show_sidebar_pane_menu(&self, event: &NSEvent, pane_id: PaneId) {
        let Some((_, working, awaiting, bare, minimized, bookmarked)) = self.pane_snapshot(pane_id) else { return };
        self.ivars().sidebar.borrow_mut().menu_pane = pane_id;
        let mut rows = vec![MenuRow::Item("Open".into(), PaneAction::Open.tag())];
        if working || awaiting {
            rows.push(MenuRow::Item("Stop".into(), PaneAction::Stop.tag()));
        }
        if bare {
            rows.push(MenuRow::Item("Start Claude here".into(), PaneAction::StartClaude.tag()));
        }
        rows.push(MenuRow::Separator);
        rows.push(MenuRow::Item("Rename\u{2026}".into(), PaneAction::Rename.tag()));
        rows.push(MenuRow::Item(
            if bookmarked { "Unbookmark" } else { "Bookmark" }.into(),
            PaneAction::ToggleBookmark.tag(),
        ));
        rows.push(if minimized {
            MenuRow::Item("Restore".into(), PaneAction::Restore.tag())
        } else {
            MenuRow::Item("Minimize".into(), PaneAction::Minimize.tag())
        });
        rows.push(MenuRow::Separator);
        rows.push(MenuRow::Item("Close".into(), PaneAction::Close.tag()));
        self.pop_up_sidebar_menu(event, &rows, objc2::sel!(sidebarPaneAction:));
    }

    /// The header's context menu: the colours, then the tab actions.
    fn show_sidebar_tab_menu(&self, event: &NSEvent, tab_idx: usize) {
        self.ivars().sidebar.borrow_mut().menu_tab = tab_idx;
        let pastilles = ["🔴", "🟠", "🟡", "🟢", "🔵", "🟣"];
        let mut rows: Vec<MenuRow> = pastilles
            .iter()
            .enumerate()
            .map(|(i, p)| MenuRow::Item((*p).into(), TabAction::Color(i).tag()))
            .collect();
        rows.push(MenuRow::Item("No colour".into(), TabAction::NoColor.tag()));
        rows.push(MenuRow::Separator);
        rows.push(MenuRow::Item("Rename tab\u{2026}".into(), TabAction::Rename.tag()));
        rows.push(MenuRow::Item("Add a pane".into(), TabAction::AddPane.tag()));
        rows.push(MenuRow::Item("Collapse others".into(), TabAction::CollapseOthers.tag()));
        rows.push(MenuRow::Separator);
        rows.push(MenuRow::Item("Close tab".into(), TabAction::Close.tag()));
        self.pop_up_sidebar_menu(event, &rows, objc2::sel!(sidebarTabAction:));
    }

    /// An `NSMenu` at the click, built like `show_tab_color_menu`: every
    /// item targets this view with `selector` and carries its tag. Blocks
    /// until the user picks or dismisses; no `tabs` borrow may be held.
    fn pop_up_sidebar_menu(&self, event: &NSEvent, rows: &[MenuRow], selector: objc2::runtime::Sel) {
        use objc2_app_kit::{NSMenu, NSMenuItem};
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let menu = NSMenu::new(mtm);
        let empty_ke = NSString::from_str("");
        for row in rows {
            match row {
                MenuRow::Separator => menu.addItem(&NSMenuItem::separatorItem(mtm)),
                MenuRow::Item(label, tag) => {
                    let title = NSString::from_str(label);
                    let item = unsafe {
                        NSMenuItem::initWithTitle_action_keyEquivalent(
                            NSMenuItem::alloc(mtm),
                            &title,
                            Some(selector),
                            &empty_ke,
                        )
                    };
                    item.setTag(*tag);
                    unsafe { item.setTarget(Some(&*self)) };
                    menu.addItem(&item);
                }
            }
        }
        let location = event.locationInWindow();
        let _ok: bool = unsafe {
            objc2::msg_send![&menu, popUpMenuPositioningItem: std::ptr::null::<NSMenuItem>(), atLocation: location, inView: self]
        };
    }

    /// A tile menu item was picked (`sidebarPaneAction:`).
    pub(super) fn sidebar_pane_menu_picked(&self, tag: isize) {
        let pane_id = self.ivars().sidebar.borrow().menu_pane;
        if let Some(action) = PaneAction::from_tag(tag) {
            self.dispatch_pane_action(pane_id, action);
        }
    }

    /// A header menu item was picked (`sidebarTabAction:`).
    pub(super) fn sidebar_tab_menu_picked(&self, tag: isize) {
        let tab_idx = self.ivars().sidebar.borrow().menu_tab;
        if let Some(action) = TabAction::from_tag(tag) {
            self.tab_action(tab_idx, action);
        }
    }

    /// Run a tile action against the Kova function behind it.
    pub(super) fn dispatch_pane_action(&self, pane_id: PaneId, action: PaneAction) {
        match action {
            PaneAction::Open | PaneAction::Restore => {
                self.focus_pane_in_window(pane_id);
            }
            PaneAction::Stop => self.interrupt_pane(pane_id),
            PaneAction::StartClaude => self.start_claude_in_pane(pane_id),
            PaneAction::Rename => {
                if self.focus_pane_in_window(pane_id) {
                    self.start_rename_pane();
                }
            }
            PaneAction::ToggleBookmark => {
                if self.focus_pane_in_window(pane_id) {
                    self.do_toggle_bookmark();
                }
            }
            PaneAction::Minimize => {
                if self.focus_pane_in_window(pane_id) {
                    self.do_minimize_pane();
                }
            }
            PaneAction::Close => self.close_pane_from_sidebar(pane_id),
        }
        self.mark_dirty();
    }

    /// Ctrl+C into the pane: what the daemon does over `send-keys`. The
    /// waiting flag and the prompt preview go with it.
    pub(super) fn interrupt_pane(&self, pane_id: PaneId) {
        let tabs = self.ivars().tabs.borrow();
        for tab in tabs.iter() {
            if let Some(pane) = tab.pane(pane_id) {
                pane.pty.write(b"\x03");
                pane.clear_awaiting();
                drop(tabs);
                self.set_transient_status("Stopped");
                return;
            }
        }
    }

    /// Type `claude` into a bare shell. Refused, like the daemon's 409, when
    /// something already runs there.
    fn start_claude_in_pane(&self, pane_id: PaneId) {
        let tabs = self.ivars().tabs.borrow();
        for tab in tabs.iter() {
            if let Some(pane) = tab.pane(pane_id) {
                if !pane.is_bare_shell() {
                    drop(tabs);
                    self.set_transient_status("Something already runs in this pane");
                    return;
                }
                pane.pty.write(b"claude\r");
                return;
            }
        }
    }

    /// Close a pane from its tile: a working agent gets the interrupt sheet
    /// first (KovaLink's `closeStateLabel` copy), then `ipc_close_pane`.
    fn close_pane_from_sidebar(&self, pane_id: PaneId) {
        let Some((title, working, ..)) = self.pane_snapshot(pane_id) else { return };
        if working {
            let mtm = unsafe { MainThreadMarker::new_unchecked() };
            let alert = NSAlert::new(mtm);
            alert.setAlertStyle(NSAlertStyle::Warning);
            alert.setMessageText(&NSString::from_str(&format!("Close {title}?")));
            alert.setInformativeText(&NSString::from_str("The agent is working right now: closing interrupts the task."));
            alert.addButtonWithTitle(&NSString::from_str("Close and interrupt"));
            alert.addButtonWithTitle(&NSString::from_str("Keep working"));
            if alert.runModal() != 1000 {
                return;
            }
        }
        if self.ipc_close_pane(pane_id) == Some(false) {
            self.set_transient_status("Cannot close the last pane");
        }
    }

    /// Run a header action.
    fn tab_action(&self, tab_idx: usize, action: TabAction) {
        match action {
            TabAction::Color(_) | TabAction::NoColor => {
                let color = match action {
                    TabAction::Color(i) => Some(i),
                    _ => None,
                };
                let mut tabs = self.ivars().tabs.borrow_mut();
                if let Some(tab) = tabs.get_mut(tab_idx) {
                    tab.color = color;
                }
            }
            TabAction::Rename => {
                self.do_switch_tab(tab_idx);
                self.start_rename_tab();
            }
            TabAction::AddPane => {
                self.do_switch_tab(tab_idx);
                self.do_split(SplitDirection::Horizontal);
            }
            TabAction::CollapseOthers => {
                let mut tabs = self.ivars().tabs.borrow_mut();
                for (i, tab) in tabs.iter_mut().enumerate() {
                    tab.collapsed = i != tab_idx;
                }
            }
            TabAction::Close => {
                self.do_switch_tab(tab_idx);
                self.do_close_tab();
            }
        }
        self.mark_dirty();
    }
}

/// Wall-clock seconds since the epoch, for the awaiting age.
fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The state a pane's tile shows. `seen` is true for the focused pane of the
/// active tab, whose unread marks are being looked at.
fn pane_state(pane: &Pane, seen: bool) -> TileState {
    let (bell, completion) = {
        let term = pane.terminal.read();
        (term.bell.load(std::sync::atomic::Ordering::Relaxed), term.unread_completion())
    };
    TileState::from_flags(PaneFlags {
        permission_prompt: pane.has_permission_prompt(),
        bell,
        completion,
        hook_unseen: pane.is_awaiting_unseen(),
        turn_end_unseen: pane.is_turn_end_unseen(),
        working: pane.is_working(),
        starting: pane.is_starting_agent(),
        idle_agent: pane.is_idle_agent(),
        seen,
    })
}

/// The shape of a pane's tile: its state, whether it is minimized or a bare
/// shell, and how many text lines it needs (awaiting: 4; unread with a
/// turn-end summary: 3; else 2).
fn pane_tile(pane: &Pane, seen: bool, content_cells: usize) -> TileLayout {
    let state = pane_state(pane, seen);
    let lines = match state {
        TileState::Awaiting => 4,
        TileState::Unread { .. } => {
            let has_summary = matches!(*pane.prompt_preview.borrow(), Some(PromptPreview::TurnEnd { ref summary, .. }) if !summary.is_empty());
            if has_summary && content_cells > 0 { 3 } else { 2 }
        }
        _ => 2,
    };
    TileLayout { state, minimized: pane.minimized, bare_shell: pane.is_bare_shell(), lines }
}

/// The lines under a tile's title beyond the secondary one: the awaiting
/// tile's question (wrapped to two lines) and detail, or the unread tile's
/// turn-end summary. Also the awaiting age.
fn tile_extra_lines(pane: &Pane, tile: &TileLayout, cells: usize, now: u64) -> (Vec<String>, u8, Option<(String, bool)>) {
    let preview = pane.prompt_preview.borrow();
    match (tile.state, preview.as_ref()) {
        (TileState::Awaiting, Some(PromptPreview::Permission { header, question, detail, since })) => {
            let mut lines = wrap_text(question, cells, 2);
            let question_lines = lines.len() as u8;
            if lines.len() == 1 {
                let detail = detail.as_deref().unwrap_or(header.as_str());
                lines.push(sidebar::truncate_title(detail, cells));
            }
            let age = now.saturating_sub(*since);
            (lines, question_lines, Some((format_age(age), age >= AGING_SECS)))
        }
        (TileState::Unread { .. }, Some(PromptPreview::TurnEnd { summary, .. })) if tile.lines == 3 => {
            (vec![sidebar::truncate_title(summary, cells)], 0, None)
        }
        _ => (Vec::new(), 0, None),
    }
}
