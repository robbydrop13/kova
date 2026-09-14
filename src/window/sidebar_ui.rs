//! The sidebar inside a window: its transient state (scroll, hover, press,
//! drag), the mouse handlers, the mode switch, and the per-frame data the
//! renderer paints from. All positions come from `sidebar::SidebarGeometry`,
//! so a click lands on the row that was drawn. The pure parts live in
//! `sidebar.rs`.

use super::*;
use super::sidebar::{
    self, display_order, edit_tail, secondary_line, short_cwd, summary_text, truncate_path,
    SidebarGeometry, SidebarHit, SidebarRowKind, SidebarSort, StateSquare,
};
use crate::config::LayoutMode;
use crate::renderer::{SidebarRowBg, SidebarRowRender};

/// Pixels of vertical travel before a pressed header lifts into a drag.
const DRAG_THRESHOLD: f32 = 3.0;
/// How often a drag parked near a list edge scrolls one header height.
const AUTOSCROLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

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

pub(super) struct SidebarState {
    scroll_y: f32,
    sort: SidebarSort,
    hovered: Option<SidebarHit>,
    /// What the mouse went down on; the action fires on mouse up inside the
    /// same target, like a button.
    pressed: Option<SidebarHit>,
    drag: Option<SidebarDrag>,
    edge_drag: bool,
    /// The mouse went down inside the sidebar: the matching mouse up is ours
    /// whatever it lands on.
    mouse_down: bool,
    /// (active tab, focused pane) the list last scrolled to show, so a focus
    /// change reveals its row exactly once and a manual scroll then sticks.
    last_reveal: Option<(usize, PaneId)>,
}

impl SidebarState {
    pub(super) fn new() -> Self {
        SidebarState {
            scroll_y: 0.0,
            sort: SidebarSort::Kova,
            hovered: None,
            pressed: None,
            drag: None,
            edge_drag: false,
            mouse_down: false,
            last_reveal: None,
        }
    }
}

/// Everything the renderer needs for one frame of the sidebar. Owned, so the
/// tick can build it before taking the renderer lock.
pub(super) struct SidebarFrame {
    pub(super) geometry: SidebarGeometry,
    pub(super) rows: Vec<SidebarRowRender>,
    pub(super) summary: String,
    pub(super) summary_color: [f32; 3],
    pub(super) sort: SidebarSort,
    pub(super) hovered: Option<SidebarHit>,
    pub(super) insertion_y: Option<f32>,
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
    /// the panes of every unfolded tab under theirs. Also returns the most
    /// urgent state of each tab (by tab index) and the total pane count.
    fn sidebar_row_kinds(&self, tabs: &[Tab], sort: SidebarSort) -> (Vec<SidebarRowKind>, usize) {
        let active_idx = self.ivars().active_tab.get();
        let tab_states: Vec<StateSquare> = tabs
            .iter()
            .enumerate()
            .map(|(ti, tab)| {
                let mut best = StateSquare::None;
                tab.for_each_pane(&mut |pane| {
                    let seen = ti == active_idx && pane.id == tab.focused_pane;
                    best = best.min(pane_square(pane, seen));
                });
                best
            })
            .collect();
        let mut kinds = Vec::new();
        let mut total_panes = 0;
        for ti in display_order(sort, &tab_states) {
            let tab = &tabs[ti];
            kinds.push(SidebarRowKind::Header { tab_idx: ti, collapsed: tab.collapsed });
            for col in &tab.columns {
                for pane in &col.panes {
                    total_panes += 1;
                    if !tab.collapsed {
                        kinds.push(SidebarRowKind::Pane { tab_idx: ti, pane_id: pane.id });
                    }
                }
            }
        }
        (kinds, total_panes)
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
    /// list follows a focus change and a drag near an edge keeps scrolling.
    pub(super) fn sidebar_frame(&self) -> Option<SidebarFrame> {
        let mut g = self.sidebar_geometry()?;
        let tabs = self.ivars().tabs.borrow();
        let active_idx = self.ivars().active_tab.get();
        let mut st = self.ivars().sidebar.borrow_mut();

        // Follow the focus: reveal the focused pane's row (its header when
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
        // A drag parked near the top or bottom of the list keeps scrolling.
        if let Some(d) = st.drag.as_mut().filter(|d| d.dragging) {
            let dir = g.autoscroll_direction(d.current_y);
            if dir != 0 && d.last_autoscroll.elapsed() >= AUTOSCROLL_INTERVAL {
                d.last_autoscroll = std::time::Instant::now();
                g.scroll_y = g.clamp_scroll(g.scroll_y + dir as f32 * g.header_h);
                self.mark_dirty();
            }
        }
        st.scroll_y = g.scroll_y;

        let home = std::env::var("HOME").unwrap_or_default();
        let bookmark_keys = self.ivars().bookmark_keys.borrow();
        let rename_tab = self.ivars().rename_tab.borrow();
        let rename_pane = self.ivars().rename_pane.borrow();
        let edit_buffer = |input: &str, cursor: usize| {
            let before: String = input.chars().take(cursor).collect();
            let after: String = input.chars().skip(cursor).collect();
            format!("{before}\u{258f}{after}")
        };

        let mut waiting = 0usize;
        let mut working = 0usize;
        for tab in tabs.iter() {
            tab.for_each_pane(&mut |pane| {
                if pane.is_awaiting() { waiting += 1; }
                if pane.is_working() { working += 1; }
            });
        }

        let row_bg = |hit: SidebarHit, alt: Option<SidebarHit>| -> SidebarRowBg {
            let is = |h: Option<SidebarHit>| h == Some(hit) || (alt.is_some() && h == alt);
            if is(st.pressed) {
                SidebarRowBg::Pressed
            } else if is(st.hovered) {
                SidebarRowBg::Hover
            } else {
                SidebarRowBg::Plain
            }
        };
        let title_cells = g.title_cells(false);
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
                    let mut squares = Vec::new();
                    let mut count = 0;
                    tab.for_each_pane(&mut |pane| {
                        count += 1;
                        squares.push(pane_square(pane, is_active && pane.id == tab.focused_pane));
                    });
                    rows.push(SidebarRowRender {
                        title,
                        secondary: String::new(),
                        number: tab_idx + 1,
                        color: tab.color,
                        active_tab: is_active,
                        focused: false,
                        square: StateSquare::group_summary(squares.into_iter()),
                        pane_count: count,
                        bg: row_bg(SidebarHit::Header(tab_idx), Some(SidebarHit::Chevron(tab_idx))),
                        bookmarked: false,
                        renaming,
                    });
                }
                SidebarRowKind::Pane { tab_idx, pane_id } => {
                    let tab = &tabs[tab_idx];
                    let Some(pane) = tab.pane(pane_id) else { continue };
                    let is_active = tab_idx == active_idx;
                    let focused = is_active && pane.id == tab.focused_pane;
                    let renaming = focused && rename_pane.is_some();
                    let mut title = if let (true, Some(rs)) = (renaming, rename_pane.as_ref()) {
                        edit_tail(&edit_buffer(&rs.input, rs.cursor), title_cells)
                    } else {
                        pane.display_title("shell")
                    };
                    if pane.minimized && !renaming {
                        title = format!("\u{229f} {title}");
                    }
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
                    let cwd_cells = title_cells.saturating_sub(what_len.map_or(0, |n| n + 3));
                    let cwd_short = truncate_path(&short_cwd(&cwd, &home), cwd_cells);
                    rows.push(SidebarRowRender {
                        title,
                        secondary: secondary_line(agent, process.as_deref(), &cwd_short),
                        number: 0,
                        color: tab.color,
                        active_tab: is_active,
                        focused,
                        square: pane_square(pane, focused),
                        pane_count: 0,
                        bg: row_bg(SidebarHit::Pane(pane_id), None),
                        bookmarked,
                        renaming,
                    });
                }
            }
        }

        let (insertion_y, lifted) = match st.drag {
            Some(d) if d.dragging => {
                let k = g.insertion_index(d.current_y);
                (
                    Some(g.insertion_line_y(k)),
                    g.row_for_header(d.tab_idx).map(|r| (r, d.current_y - d.grab_offset)),
                )
            }
            _ => (None, None),
        };

        Some(SidebarFrame {
            summary: summary_text(waiting, working),
            summary_color: if waiting > 0 { sidebar::AWAITING_COLOR } else { sidebar::WORKING_COLOR },
            sort: st.sort,
            hovered: st.hovered,
            insertion_y,
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
                    self.ivars().sidebar.borrow_mut().pressed = Some(hit);
                }
            }
            SidebarHit::Chevron(_) | SidebarHit::SortToggle | SidebarHit::ModeButton => {
                self.ivars().sidebar.borrow_mut().pressed = Some(hit);
            }
            SidebarHit::Empty => {}
        }
        self.mark_dirty();
        true
    }

    /// Mouse drag that started in the sidebar: edge resize, header drag, or
    /// a press that leaves its row (which cancels the click).
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

    /// Mouse up after a sidebar mouse down: fire the click, drop the tab, or
    /// remember the new width.
    pub(super) fn sidebar_mouse_up(&self, event: &NSEvent) -> bool {
        let (edge, drag, pressed) = {
            let mut st = self.ivars().sidebar.borrow_mut();
            if !st.mouse_down {
                return false;
            }
            st.mouse_down = false;
            (
                std::mem::take(&mut st.edge_drag),
                st.drag.take(),
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
            SidebarHit::Pane(pane_id) => {
                self.focus_pane_in_window(pane_id);
            }
            SidebarHit::SortToggle => {
                let mut st = self.ivars().sidebar.borrow_mut();
                st.sort = st.sort.toggled();
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

    /// Hover tracking and cursor while the mouse moves over the sidebar.
    /// Returns false when the point is not ours.
    pub(super) fn sidebar_mouse_moved(&self, event: &NSEvent) -> bool {
        let (px, py) = self.event_to_pixel(event);
        let hit = self.sidebar_geometry().and_then(|g| g.hit(px, py));
        let hovered = match hit {
            Some(
                SidebarHit::Header(_)
                | SidebarHit::Chevron(_)
                | SidebarHit::Pane(_)
                | SidebarHit::SortToggle
                | SidebarHit::ModeButton,
            ) => hit,
            _ => None,
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
            Some(_) => {
                NSCursor::arrowCursor().set();
                // A pane status bar scrolled under the sidebar must not keep
                // its tooltip up.
                if let Some(renderer) = self.ivars().renderer.get() {
                    let mut r = renderer.write();
                    if r.active_tooltip.is_some() {
                        r.active_tooltip = None;
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

    /// Right click on a group header opens the tab colour menu. Returns
    /// false when the point is not ours.
    pub(super) fn sidebar_right_click(&self, px: f32, py: f32, event: &NSEvent) -> bool {
        let Some(g) = self.sidebar_geometry() else { return false };
        match g.hit(px, py) {
            None => false,
            Some(SidebarHit::Header(tab_idx)) | Some(SidebarHit::Chevron(tab_idx)) => {
                self.show_tab_color_menu(event, tab_idx);
                true
            }
            Some(_) => true,
        }
    }
}

/// The square a pane row shows. `seen` is true for the focused pane of the
/// active tab, whose bell and completion are being looked at.
fn pane_square(pane: &Pane, seen: bool) -> StateSquare {
    let (bell, completion) = {
        let term = pane.terminal.read();
        (term.bell.load(std::sync::atomic::Ordering::Relaxed), term.unread_completion())
    };
    StateSquare::from_flags(
        pane.is_awaiting(),
        bell,
        completion,
        pane.is_working(),
        pane.is_idle_agent(),
        seen,
    )
}
