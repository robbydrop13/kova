//! The sidebar from the window's side: placing the AppKit sidebar next to the
//! Metal view (`apply_layout`), the mode switch, the per-tick sync that reads
//! the tabs into a `SidebarModel` and hands it to the view, the actions the
//! view calls back (open, stop, close, rename, bookmark, minimize, start
//! Claude, resume, add a pane, reorder) and the context menus. The drawing
//! and the mouse live in `sidebar_view.rs`.

use super::*;
use super::sidebar::{self, swap_chain, PaneFlags, SidebarSort};
use super::sidebar_model::{header_title, PaneFacts, SidebarModel, TabFacts};
use crate::config::LayoutMode;

/// How long `✓ All caught up` stays up once the last unread was read.
const CAUGHT_UP_SECS: f32 = 1.6;

/// What a tile's context menu can do, dispatched by the item's tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaneAction {
    Open,
    Stop,
    StartClaude,
    /// Run the resume line waiting at the pane's prompt.
    Resume,
    Rename,
    ToggleBookmark,
    Minimize,
    Restore,
    Close,
}

impl PaneAction {
    const ALL: [PaneAction; 9] = [
        PaneAction::Open,
        PaneAction::Stop,
        PaneAction::StartClaude,
        PaneAction::Resume,
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
    /// A colour choice: its label, tag, swatch (`None` for the grey ring of
    /// "No colour") and whether it is the current one (check mark).
    Swatch(String, isize, Option<[f32; 3]>, bool),
    Separator,
}

/// The six tab colours, in `TAB_COLORS` order, as the colour menu names them.
pub(super) const TAB_COLOR_NAMES: [&str; 6] = ["Red", "Orange", "Yellow", "Green", "Blue", "Violet"];

pub(super) struct SidebarState {
    sort: SidebarSort,
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
            sort: SidebarSort::Kova,
            last_reveal: None,
            last_unread: None,
            caught_up_frames: 0,
            menu_pane: 0,
            menu_tab: 0,
        }
    }
}

impl KovaView {
    /// Whether the sidebar is on screen: the mode says so and the window is
    /// wide enough to keep a split-wide column next to it. Otherwise the
    /// window shows the tab bar until a resize makes room again, without
    /// touching the setting.
    pub(super) fn sidebar_active(&self) -> bool {
        self.ivars().sidebar_shown.get()
    }

    pub(super) fn sidebar_sort(&self) -> SidebarSort {
        self.ivars().sidebar.borrow().sort
    }

    pub fn set_sidebar_sort(&self, sort: SidebarSort) {
        self.ivars().sidebar.borrow_mut().sort = sort;
    }

    /// Place the sidebar view and this view side by side in their container
    /// (or hide the sidebar and take the whole width), from the current mode,
    /// width and container size. Setting this view's frame runs
    /// `handle_resize`, which hands every pane its new size.
    pub(super) fn apply_layout(&self) {
        let Some(container) = (unsafe { self.superview() }) else { return };
        let Some(sidebar) = self.ivars().sidebar_view.get() else { return };
        let bounds = container.bounds();
        let width = sidebar::width_pt() as f64;
        let min_split = self.ivars().config.get().map(|c| c.splits.min_width).unwrap_or(300.0) as f64;
        let wanted = sidebar::layout_mode() == LayoutMode::Sidebar;
        let (show, sidebar_frame, my_frame) = split_layout(bounds, width, min_split, wanted);
        self.ivars().sidebar_shown.set(show);
        log::debug!(
            "apply_layout: container {}x{} sidebar {} at x={} w={} kova at x={} w={} h={}",
            bounds.size.width,
            bounds.size.height,
            if show { "shown" } else { "hidden" },
            sidebar_frame.origin.x,
            sidebar_frame.size.width,
            my_frame.origin.x,
            my_frame.size.width,
            my_frame.size.height
        );
        if sidebar.isHidden() == show {
            sidebar.setHidden(!show);
        }
        if !rect_eq(sidebar.frame(), sidebar_frame) {
            sidebar.setFrame(sidebar_frame);
        }
        if !rect_eq(self.frame(), my_frame) {
            self.setFrame(my_frame);
        }
        self.mark_dirty();
    }

    /// Switch the whole app between the tab bar and the sidebar, remember
    /// it, and relayout every window.
    pub(super) fn set_layout_mode(&self, mode: LayoutMode) {
        if sidebar::layout_mode() == mode {
            return;
        }
        sidebar::set_layout_mode(mode);
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let ad = crate::app::app_delegate(mtm);
        // Copy the list: `apply_layout` resizes every window, and nothing
        // that runs under it may find the list borrowed.
        let windows: Vec<_> = ad.ivars().windows.borrow().clone();
        for win in &windows {
            if let Some(view) = crate::app::kova_view(win) {
                view.apply_layout();
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

    // ---------------------------------------------------------------
    // Per-tick sync
    // ---------------------------------------------------------------

    /// Read the tabs into a model and hand it to the sidebar view, which
    /// redraws only when something changed. Also where the list follows a
    /// focus change and the caught-up flash counts down. Runs once per tick.
    pub(super) fn sync_sidebar(&self) {
        let Some(view) = self.ivars().sidebar_view.get() else { return };
        if !self.sidebar_active() {
            return;
        }
        // Cmd+J's tiers across every window, for the Next pill.
        let attention = self.collect_attention();
        let unread = attention.unread.len();
        let idle = attention.idle_agent.len();
        let (flashing, sort) = {
            let mut st = self.ivars().sidebar.borrow_mut();
            if st.last_unread.is_some_and(|before| before > 0) && unread == 0 {
                let fps = self.ivars().config.get().map(|c| c.terminal.fps).unwrap_or(60) as f32;
                st.caught_up_frames = (fps * CAUGHT_UP_SECS) as u32;
            }
            st.last_unread = Some(unread);
            if st.caught_up_frames > 0 {
                st.caught_up_frames -= 1;
            }
            (st.caught_up_frames > 0, st.sort)
        };
        let facts = self.sidebar_tab_facts();
        let model = SidebarModel::build(&facts, sort, unread, idle, flashing, now_epoch_secs());
        view.set_model(model);

        // Follow the focus: reveal the focused pane's tile (its header when
        // the group is folded), once per focus change.
        let key = {
            let tabs = self.ivars().tabs.borrow();
            let active_idx = self.ivars().active_tab.get();
            tabs.get(active_idx).map(|tab| (active_idx, tab.focused_pane))
        };
        if let Some(key) = key {
            let mut st = self.ivars().sidebar.borrow_mut();
            if st.last_reveal != Some(key) {
                st.last_reveal = Some(key);
                drop(st);
                view.reveal(key.1, key.0);
            }
        }
    }

    /// Every read of the tabs the model is built from.
    fn sidebar_tab_facts(&self) -> Vec<TabFacts> {
        let tabs = self.ivars().tabs.borrow();
        let active_idx = self.ivars().active_tab.get();
        let bookmark_keys = self.ivars().bookmark_keys.borrow();
        let rename_tab = self.ivars().rename_tab.borrow();
        let rename_pane = self.ivars().rename_pane.borrow();
        let edit_buffer = |input: &str, cursor: usize| {
            let before: String = input.chars().take(cursor).collect();
            let after: String = input.chars().skip(cursor).collect();
            format!("{before}\u{258f}{after}")
        };
        tabs.iter()
            .enumerate()
            .map(|(ti, tab)| {
                let is_active = ti == active_idx;
                let renaming = is_active && rename_tab.is_some();
                let mut panes = Vec::new();
                for (column, col) in tab.columns.iter().enumerate() {
                    for pane in &col.panes {
                        let focused = is_active && pane.id == tab.focused_pane;
                        let edit = match rename_pane.as_ref() {
                            Some(rs) if focused => Some(edit_buffer(&rs.input, rs.cursor)),
                            _ => None,
                        };
                        let (cwd, bookmarked) = {
                            // The directory the shell reports (OSC 7); a shell
                            // without that integration is asked directly (one
                            // `proc_pidinfo`, only for those panes).
                            let cwd = pane.terminal.read().cwd.clone().or_else(|| pane.cwd()).unwrap_or_default();
                            let bookmarked = match pane.agent_session.borrow().as_ref() {
                                Some(session) => bookmark_keys.contains(&session.id),
                                None => !cwd.is_empty() && bookmark_keys.contains(&cwd),
                            };
                            (cwd, bookmarked)
                        };
                        let restored = pane.restored_session().map(|(agent, _)| agent);
                        panes.push(PaneFacts {
                            pane_id: pane.id,
                            column,
                            flags: pane_flags(pane, focused),
                            minimized: pane.minimized,
                            bare_shell: pane.is_bare_shell(),
                            session_name: pane.agent_session_name(),
                            custom_title: pane.custom_title.clone(),
                            osc_title: pane.osc_title(),
                            edit,
                            agent: pane.agent_kind().or(restored).map(|a| a.as_str().to_string()),
                            resumable: restored.is_some(),
                            process: pane.fg_process().map(|p| p.name),
                            cwd,
                            bookmarked,
                            focused,
                            preview: pane.prompt_preview.borrow().clone(),
                        });
                    }
                }
                // The header: the tab's own name, else its focused pane's name
                // or project (`header_title`: never a raw directory).
                let title = match (renaming, rename_tab.as_ref()) {
                    (true, Some(rs)) => edit_buffer(&rs.input, rs.cursor),
                    _ => tab.custom_title.clone().unwrap_or_else(|| {
                        panes.iter().find(|p| p.pane_id == tab.focused_pane).map(header_title).unwrap_or_else(|| "shell".to_string())
                    }),
                };
                TabFacts {
                    tab_idx: ti,
                    tab_id: tab.id,
                    title,
                    color: tab.color,
                    active: is_active,
                    collapsed: tab.collapsed,
                    renaming,
                    panes,
                }
            })
            .collect()
    }

    // ---------------------------------------------------------------
    // Actions the view calls back
    // ---------------------------------------------------------------

    pub(super) fn sidebar_toggle_collapsed(&self, tab_idx: usize) {
        let mut tabs = self.ivars().tabs.borrow_mut();
        if let Some(tab) = tabs.get_mut(tab_idx) {
            tab.collapsed = !tab.collapsed;
        }
    }

    /// A click on a header body: switch to the tab, or fold the active one.
    pub(super) fn sidebar_header_clicked(&self, tab_idx: usize) {
        if tab_idx == self.ivars().active_tab.get() {
            self.sidebar_toggle_collapsed(tab_idx);
        } else {
            self.do_switch_tab(tab_idx);
        }
    }

    pub(super) fn sidebar_toggle_sort(&self) {
        let mut st = self.ivars().sidebar.borrow_mut();
        st.sort = st.sort.toggled();
    }

    /// The Next pill: `Nothing to read` is not a button, only a pill with a
    /// badge jumps.
    pub(super) fn sidebar_next_pill_clicked(&self) {
        let sets = self.collect_attention();
        if sidebar::NextPill::of(sets.unread.len(), sets.idle_agent.len(), false).clickable() {
            self.do_focus_next_attention();
        }
    }

    /// Drop a dragged header at insertion position `k` (before the k-th
    /// group, or at the end).
    pub(super) fn sidebar_reorder_tab(&self, from: usize, k: usize) {
        let to = if k > from { k - 1 } else { k };
        let tab_id = {
            let tabs = self.ivars().tabs.borrow();
            if to == from || from >= tabs.len() {
                return;
            }
            tabs[from].id
        };
        self.ipc_move_tab(tab_id, to);
        self.mark_dirty();
    }

    /// Drop a dragged tile: `ids` are the panes of its column run, the tile
    /// moves from `from` to `to`, replayed as adjacent swaps through
    /// `Tab::swap_panes`, the primitive behind the `swap-pane` IPC command.
    pub(super) fn sidebar_drop_pane(&self, tab_idx: usize, ids: &[PaneId], from: usize, to: usize) {
        let chain = swap_chain(ids.len(), from, to);
        if chain.is_empty() {
            return;
        }
        {
            let mut tabs = self.ivars().tabs.borrow_mut();
            let Some(tab) = tabs.get_mut(tab_idx) else { return };
            for (a, b) in chain {
                tab.swap_panes(ids[a], ids[b], crate::pane::NavDirection::Down);
            }
            tab.mark_all_dirty();
        }
        self.resize_all_panes();
        self.mark_dirty();
    }

    // ---------------------------------------------------------------
    // Menus
    // ---------------------------------------------------------------

    /// The pane `id` lives in, with the reads the menus and actions need.
    fn pane_snapshot(&self, pane_id: PaneId) -> Option<(String, bool, bool, bool, bool, bool, bool)> {
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
                    pane.restored_session().is_some(),
                    pane.minimized,
                    bookmarked,
                ));
            }
        }
        None
    }

    /// The tile's context menu, KovaLink's swipes and sheets as items,
    /// popped up at `location` in `view`.
    pub(super) fn show_sidebar_pane_menu(&self, view: &objc2_app_kit::NSView, location: CGPoint, pane_id: PaneId) {
        let Some((_, working, awaiting, bare, resumable, minimized, bookmarked)) = self.pane_snapshot(pane_id) else { return };
        self.ivars().sidebar.borrow_mut().menu_pane = pane_id;
        let mut rows = vec![MenuRow::Item("Open".into(), PaneAction::Open.tag())];
        if working || awaiting {
            rows.push(MenuRow::Item("Stop".into(), PaneAction::Stop.tag()));
        }
        if resumable {
            rows.push(MenuRow::Item("Resume the session".into(), PaneAction::Resume.tag()));
        } else if bare {
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
        self.pop_up_sidebar_menu(view, location, &rows, objc2::sel!(sidebarPaneAction:));
    }

    /// The header's context menu: the colours, then the tab actions.
    pub(super) fn show_sidebar_tab_menu(&self, view: &objc2_app_kit::NSView, location: CGPoint, tab_idx: usize) {
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
        self.pop_up_sidebar_menu(view, location, &rows, objc2::sel!(sidebarTabAction:));
    }

    /// The colour picker under a header's dot: the six tab colours with
    /// their swatches, then `No colour`, the current one checked. Picks go
    /// through `sidebarTabAction:` like the header menu's colour items.
    pub(super) fn show_sidebar_color_menu(&self, view: &objc2_app_kit::NSView, location: CGPoint, tab_idx: usize) {
        self.ivars().sidebar.borrow_mut().menu_tab = tab_idx;
        // Bind the current colour before the menu blocks: no `tabs` borrow
        // may be held while it runs.
        let current = self.ivars().tabs.borrow().get(tab_idx).and_then(|t| t.color);
        let mut rows: Vec<MenuRow> = TAB_COLOR_NAMES
            .iter()
            .enumerate()
            .map(|(i, name)| MenuRow::Swatch((*name).into(), TabAction::Color(i).tag(), Some(crate::renderer::TAB_COLORS[i]), current == Some(i)))
            .collect();
        rows.push(MenuRow::Swatch("No colour".into(), TabAction::NoColor.tag(), None, current.is_none()));
        self.pop_up_sidebar_menu(view, location, &rows, objc2::sel!(sidebarTabAction:));
    }

    /// An `NSMenu` at `location` in `view`, built like `show_tab_color_menu`:
    /// every item targets this view with `selector` and carries its tag.
    /// Blocks until the user picks or dismisses; no `tabs` borrow may be held.
    fn pop_up_sidebar_menu(&self, view: &objc2_app_kit::NSView, location: CGPoint, rows: &[MenuRow], selector: objc2::runtime::Sel) {
        use objc2_app_kit::{NSMenu, NSMenuItem};
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let menu = NSMenu::new(mtm);
        let empty_ke = NSString::from_str("");
        for row in rows {
            let (label, tag, swatch) = match row {
                MenuRow::Separator => {
                    menu.addItem(&NSMenuItem::separatorItem(mtm));
                    continue;
                }
                MenuRow::Item(label, tag) => (label, *tag, None),
                MenuRow::Swatch(label, tag, color, checked) => (label, *tag, Some((*color, *checked))),
            };
            let title = NSString::from_str(label);
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &title,
                    Some(selector),
                    &empty_ke,
                )
            };
            item.setTag(tag);
            unsafe { item.setTarget(Some(&*self)) };
            if let Some((color, checked)) = swatch {
                item.setImage(Some(&super::sidebar_view::swatch_image(color)));
                if checked {
                    item.setState(unsafe { objc2_app_kit::NSControlStateValueOn });
                }
            }
            menu.addItem(&item);
        }
        let _ok: bool = unsafe {
            objc2::msg_send![&menu, popUpMenuPositioningItem: std::ptr::null::<NSMenuItem>(), atLocation: location, inView: view]
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
            self.run_tab_action(tab_idx, action);
        }
    }

    // ---------------------------------------------------------------
    // Actions and the Kova functions behind them
    // ---------------------------------------------------------------

    /// Run a tile action against the Kova function behind it.
    pub(super) fn dispatch_pane_action(&self, pane_id: PaneId, action: PaneAction) {
        match action {
            PaneAction::Open | PaneAction::Restore => {
                self.focus_pane_in_window(pane_id);
            }
            PaneAction::Stop => self.interrupt_pane(pane_id),
            PaneAction::StartClaude => self.start_claude_in_pane(pane_id),
            PaneAction::Resume => self.resume_in_pane(pane_id),
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

    /// Run the resume line a restored pane holds (`Pane::restored_session`).
    /// The line is rebuilt by `resume_command`, which refuses an id that could
    /// carry a second command, and retyped after a Ctrl+U: the pre-typed one
    /// may still sit at the prompt or have been cleared. Refused, like `Start
    /// Claude`, when something already runs there.
    fn resume_in_pane(&self, pane_id: PaneId) {
        let tabs = self.ivars().tabs.borrow();
        for tab in tabs.iter() {
            if let Some(pane) = tab.pane(pane_id) {
                let command = pane.restored_session().and_then(|(agent, id)| {
                    crate::agent_session::resume_command(agent, &id, pane.last_command().as_deref())
                });
                match command {
                    Some(command) => pane.pty.write(format!("\x15{command}\r").as_bytes()),
                    None => {
                        drop(tabs);
                        self.set_transient_status("Nothing to resume in this pane");
                    }
                }
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
    pub(super) fn run_tab_action(&self, tab_idx: usize, action: TabAction) {
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

/// How the container `bounds` split between the sidebar and the Metal view:
/// whether the sidebar shows (wanted, and the window keeps at least
/// `min_split` for the terminal next to it), its frame, and the Metal view's.
/// Both frames sit at y = 0 and take the full height; the Metal view takes
/// the whole container when the sidebar is hidden.
fn split_layout(bounds: CGRect, width: f64, min_split: f64, wanted: bool) -> (bool, CGRect, CGRect) {
    let show = wanted && bounds.size.width - width >= min_split;
    let sidebar_frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize { width, height: bounds.size.height },
    };
    let my_frame = if show {
        CGRect {
            origin: CGPoint { x: width, y: 0.0 },
            size: CGSize { width: (bounds.size.width - width).max(1.0), height: bounds.size.height },
        }
    } else {
        bounds
    };
    (show, sidebar_frame, my_frame)
}

fn rect_eq(a: CGRect, b: CGRect) -> bool {
    (a.origin.x - b.origin.x).abs() < 0.01
        && (a.origin.y - b.origin.y).abs() < 0.01
        && (a.size.width - b.size.width).abs() < 0.01
        && (a.size.height - b.size.height).abs() < 0.01
}

/// Wall-clock seconds since the epoch, for the awaiting age.
fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The flags a pane's tile state is derived from. `seen` is true for the
/// focused pane of the active tab, whose unread marks are being looked at.
fn pane_flags(pane: &Pane, seen: bool) -> PaneFlags {
    let (bell, completion) = {
        let term = pane.terminal.read();
        (term.bell.load(std::sync::atomic::Ordering::Relaxed), term.unread_completion())
    };
    PaneFlags {
        permission_prompt: pane.has_permission_prompt(),
        bell,
        completion,
        hook_unseen: pane.is_awaiting_unseen(),
        turn_end_unseen: pane.is_turn_end_unseen(),
        working: pane.is_working(),
        starting: pane.is_starting_agent(),
        idle_agent: pane.is_idle_agent(),
        seen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(w: f64, h: f64) -> CGRect {
        CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: w, height: h } }
    }

    #[test]
    fn the_colour_menu_names_every_tab_colour_and_round_trips_its_tags() {
        assert_eq!(TAB_COLOR_NAMES.len(), crate::renderer::TAB_COLORS.len());
        for i in 0..TAB_COLOR_NAMES.len() {
            assert_eq!(TabAction::from_tag(TabAction::Color(i).tag()), Some(TabAction::Color(i)));
        }
        assert_eq!(TabAction::from_tag(TabAction::NoColor.tag()), Some(TabAction::NoColor));
        assert!(TAB_COLOR_NAMES.iter().all(|n| n.is_ascii()));
    }

    #[test]
    fn the_split_follows_the_container_size() {
        let (show, sidebar, kova) = split_layout(bounds(2000.0, 1230.0), 280.0, 300.0, true);
        assert!(show);
        assert!(rect_eq(sidebar, CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 280.0, height: 1230.0 } }));
        assert!(rect_eq(kova, CGRect { origin: CGPoint { x: 280.0, y: 0.0 }, size: CGSize { width: 1720.0, height: 1230.0 } }));
        assert_eq!(sidebar.size.width + kova.size.width, 2000.0);

        let (show, _, kova) = split_layout(bounds(1230.0, 780.0), 280.0, 300.0, true);
        assert!(show);
        assert!(rect_eq(kova, CGRect { origin: CGPoint { x: 280.0, y: 0.0 }, size: CGSize { width: 950.0, height: 780.0 } }));
    }

    #[test]
    fn the_sidebar_hides_in_tab_mode_and_when_the_window_is_too_narrow() {
        let (show, _, kova) = split_layout(bounds(2000.0, 1230.0), 280.0, 300.0, false);
        assert!(!show);
        assert!(rect_eq(kova, bounds(2000.0, 1230.0)));

        let (show, _, kova) = split_layout(bounds(500.0, 600.0), 280.0, 300.0, true);
        assert!(!show);
        assert!(rect_eq(kova, bounds(500.0, 600.0)));

        let (show, ..) = split_layout(bounds(580.0, 600.0), 280.0, 300.0, true);
        assert!(show);
    }
}
