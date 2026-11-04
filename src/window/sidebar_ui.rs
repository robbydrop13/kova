//! The sidebar from the window's side: placing the AppKit sidebar next to the
//! Metal view (`apply_layout`), the mode switch, the per-tick sync that reads
//! the tabs into a `SidebarModel` and hands it to the view, the actions the
//! view calls back (open, stop, close, rename, bookmark, minimize, start
//! Claude, resume, add a pane, reorder) and the context menus. The drawing
//! and the mouse live in `sidebar_view.rs`.

use super::*;
use super::sidebar::{self, swap_chain, PaneFlags, SidebarSort};
use super::needs::{self, AskNeed, NeedInput, NeedItem, NeedKind, PaneNeed, RoutineNeed};
use super::sidebar_asks::{self, AskCardVm, AsksHit, AsksState, Highlight};
use super::sidebar_agents;
use super::sidebar_model::{asks_summary, header_title, next_pill, PaneFacts, RoutineVm, SidebarList, SidebarModel, TabFacts};
use super::sidebar_sched::{self, SchedButton, SchedHit, SchedState};
use crate::config::LayoutMode;
use crate::renderer::{TAB_COLORS, TAB_COLOR_NAMES};

/// The tick's need list, built once and shared by every window: building it
/// scans every window's panes, so a window that redid it made the frame cost
/// grow with the square of the window count.
static NEEDS_NOW: std::sync::Mutex<Option<std::sync::Arc<NeedsNow>>> = std::sync::Mutex::new(None);

/// One tick's need list, and what the badges and Cmd+J derive from it.
struct NeedsNow {
    /// Bumped only when the list changes: a window whose copy carries the same
    /// number has nothing to copy or redraw.
    generation: u64,
    items: Vec<NeedItem>,
    waits_on_you: u32,
    routine_failed: bool,
}

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
    /// Mark read when unread, unread by hand otherwise (Cmd+U on the tile).
    ToggleUnread,
    Close,
}

impl PaneAction {
    const ALL: [PaneAction; 10] = [
        PaneAction::Open,
        PaneAction::Stop,
        PaneAction::StartClaude,
        PaneAction::Resume,
        PaneAction::Rename,
        PaneAction::ToggleBookmark,
        PaneAction::Minimize,
        PaneAction::Restore,
        PaneAction::ToggleUnread,
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

/// Menu tags: a colour is its palette index, the other actions sit past
/// the palette.
const TAG_NO_COLOR: isize = 20;

impl TabAction {
    fn from_tag(tag: isize) -> Option<Self> {
        match tag {
            t if (0..TAB_COLORS.len() as isize).contains(&t) => Some(TabAction::Color(t as usize)),
            TAG_NO_COLOR => Some(TabAction::NoColor),
            21 => Some(TabAction::Rename),
            22 => Some(TabAction::AddPane),
            23 => Some(TabAction::CollapseOthers),
            24 => Some(TabAction::Close),
            _ => None,
        }
    }

    fn tag(self) -> isize {
        match self {
            TabAction::Color(i) => i as isize,
            TabAction::NoColor => TAG_NO_COLOR,
            TabAction::Rename => 21,
            TabAction::AddPane => 22,
            TabAction::CollapseOthers => 23,
            TabAction::Close => 24,
        }
    }
}

/// The colour rows of a menu: every palette colour with its swatch, then
/// `No colour`, the current one checked.
fn color_rows(current: Option<usize>) -> Vec<MenuRow> {
    let mut rows: Vec<MenuRow> = TAB_COLOR_NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| MenuRow::Swatch((*name).into(), TabAction::Color(i).tag(), Some(TAB_COLORS[i]), current == Some(i)))
        .collect();
    rows.push(MenuRow::Swatch("No colour".into(), TabAction::NoColor.tag(), None, current.is_none()));
    rows
}

/// A menu row: a label and its tag, or a separator.
enum MenuRow {
    Item(String, isize),
    /// A colour choice: its label, tag, swatch (`None` for the grey ring of
    /// "No colour") and whether it is the current one (check mark).
    Swatch(String, isize, Option<[f32; 3]>, bool),
    Separator,
}


pub(super) struct SidebarState {
    pub(super) sort: SidebarSort,
    /// Which list the switcher at the top of the sidebar has selected.
    pub(super) list: SidebarList,
    /// The routine clicked open, by name. One at a time: the body is long,
    /// and two open at once turns the list into a wall of prompt.
    open_routine: Option<String>,
    /// (active tab, focused pane) the list last scrolled to show, so a focus
    /// change reveals its row exactly once and a manual scroll then sticks.
    last_reveal: Option<(usize, PaneId)>,
    /// The pane a context menu is open for.
    menu_pane: PaneId,
    /// The tab a context menu is open for.
    menu_tab: usize,
    /// What waits on the user, in the order `Next` walks it
    /// (`needs::need_items`), built once per app tick and shared by every
    /// window, whether the sidebar is on screen or not: the badges and Cmd+J
    /// read the same list.
    pub(super) needs: Vec<NeedItem>,
    /// Which build of the shared list `needs` holds, so a tick that changed
    /// nothing copies nothing.
    needs_generation: u64,
    /// `needs::waits_on_you`, the badge on the Asks icon.
    pub(super) waits_on_you: u32,
    /// A scheduled routine's last run failed, the badge on the Scheduled icon.
    pub(super) routine_failed: bool,
    /// `Show all` on the `Needs you` block: every row rather than the four
    /// it folds to.
    pub(super) needs_all: bool,
    /// The Asks list's cards and transient state, read from the board while
    /// the Asks list is up.
    pub(super) asks: AsksState,
    /// The ask a highlight already scrolled to, so it reveals its card once.
    last_revealed_ask: Option<String>,
    /// The failed routine Cmd+J already showed, so it shows it once rather
    /// than on every press until the routine runs clean again.
    pub(super) routine_shown: Option<String>,
    /// The Scheduled view: folded sections, the delete confirmation, the
    /// actions in flight and the words of the ones that failed.
    pub(super) sched: SchedState,
    /// The Agents view's selected stat tile, folding and reopens in flight.
    pub(super) agents: sidebar_agents::AgentsState,
    /// When each pane entered the state it is in now, the view's own clock.
    pub(super) agents_clock: sidebar_agents::AgentsClock,
}

impl SidebarState {
    pub(super) fn new() -> Self {
        SidebarState {
            sort: SidebarSort::Kova,
            list: SidebarList::default(),
            open_routine: None,
            last_reveal: None,
            menu_pane: 0,
            menu_tab: 0,
            needs: Vec::new(),
            needs_generation: 0,
            waits_on_you: 0,
            routine_failed: false,
            needs_all: false,
            asks: AsksState::default(),
            last_revealed_ask: None,
            routine_shown: None,
            sched: SchedState::default(),
            agents: sidebar_agents::AgentsState::default(),
            agents_clock: sidebar_agents::AgentsClock::default(),
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
        let panel = self.ivars().side_panel.get();
        let panel_wanted = panel.is_some_and(|p| p.wanted());
        let (show, sidebar_frame, my_frame, panel_frame) =
            split_frames(bounds, width, min_split, wanted, panel_wanted, super::side_panel::fraction());
        self.ivars().sidebar_shown.set(show);
        log::debug!(
            "apply_layout: container {}x{} sidebar {} at x={} w={} kova at x={} w={} h={} panel {:?}",
            bounds.size.width,
            bounds.size.height,
            if show { "shown" } else { "hidden" },
            sidebar_frame.origin.x,
            sidebar_frame.size.width,
            my_frame.origin.x,
            my_frame.size.width,
            my_frame.size.height,
            panel_frame.map(|f| (f.origin.x, f.size.width))
        );
        if sidebar.isHidden() == show {
            sidebar.setHidden(!show);
        }
        if !rect_eq(sidebar.frame(), sidebar_frame) {
            sidebar.setFrame(sidebar_frame);
        }
        if let Some(panel) = panel {
            match panel_frame {
                Some(frame) => {
                    if !rect_eq(panel.frame(), frame) {
                        panel.setFrame(frame);
                    }
                    if panel.isHidden() {
                        panel.setHidden(false);
                    }
                }
                None => {
                    if !panel.isHidden() {
                        panel.setHidden(true);
                        // The keyboard goes back to the terminal with the panel.
                        if let Some(win) = self.window()
                            && win.firstResponder().is_some_and(|r| panel.holds(&r))
                        {
                            win.makeFirstResponder(Some(self));
                        }
                    }
                }
            }
        }
        let resized = !rect_eq(self.frame(), my_frame);
        if resized {
            // Before the frame changes: the resize reads the pinned widths.
            self.side_panel_pins(panel_frame.map(|f| f.size.width), my_frame.size.width);
            self.setFrame(my_frame);
            let screen_w = self.drawable_viewport().width;
            let mut tabs = self.ivars().tabs.borrow_mut();
            if let Some(tab) = tabs.get_mut(self.ivars().active_tab.get()) {
                let focused = tab.focused_pane;
                self.scroll_to_reveal_pane(tab, focused, screen_w);
            }
        }
        self.mark_dirty();
    }

    /// The side panel does not squash the panes: while it is open, every tab
    /// keeps the width it would have without it and scrolls horizontally
    /// (`panel_pin`). A width the user set themselves is left alone; the pins
    /// Kova set come off when the panel closes.
    fn side_panel_pins(&self, panel_w: Option<f64>, term_w: f64) {
        let min_w = self.ivars().config.get().map(|c| c.splits.min_width).unwrap_or(300.0);
        let scale = self.backing_scale();
        let wide = (term_w + panel_w.unwrap_or(0.0)) as f32;
        let mut pins = self.ivars().panel_pins.borrow_mut();
        let mut tabs = self.ivars().tabs.borrow_mut();
        let mut next = Vec::new();
        for tab in tabs.iter_mut() {
            let ours = pins.iter().find(|(id, _)| *id == tab.id).map(|(_, v)| *v);
            let now = tab.virtual_width_override;
            let wide_vw = {
                tab.virtual_width_override = 0.0;
                let vw = tab.virtual_width(wide, min_w);
                tab.virtual_width_override = now;
                vw
            };
            let (set, keep) = panel_pin(now, ours, wide_vw, term_w as f32, scale, panel_w.is_some());
            tab.virtual_width_override = set;
            if let Some(v) = keep {
                next.push((tab.id, v));
            }
        }
        *pins = next;
    }

    /// Show `target` in this window's side panel.
    pub(crate) fn open_in_side_panel(&self, target: super::side_panel::PanelTarget) {
        if let Some(panel) = self.ivars().side_panel.get() {
            panel.open_target(target);
        }
    }

    /// A decision sheet was answered: a panel showing it shows what was sent.
    pub(crate) fn side_panel_sheet_closed(&self, id: &str, title: &str, summary: Option<&str>) {
        if let Some(panel) = self.ivars().side_panel.get() {
            panel.sheet_closed(id, title, summary);
        }
    }

    /// The View menu's toggle. Nothing to reopen yet: says so in the log.
    pub(super) fn toggle_side_panel(&self) {
        if let Some(panel) = self.ivars().side_panel.get()
            && !panel.toggle()
        {
            log::info!("side panel: nothing to show yet (Cmd+click a link or a .md file, or open a decision)");
        }
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
    /// Build the need list for this tick and publish it for every window: the
    /// list scans every window's panes, so a window that rebuilt it made the
    /// frame cost grow with the square of the window count. The app timer
    /// calls this once, before ticking the windows.
    pub(crate) fn refresh_needs_shared(&self) {
        let input = self.need_input(now_epoch_secs());
        let items = needs::need_items(&input);
        let waits_on_you = needs::waits_on_you(&items);
        let routine_failed = items.iter().any(|item| item.kind == NeedKind::FailedRoutine);
        let mut slot = NEEDS_NOW.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // Another list bumps the generation, so each window copies it once; an
        // unchanged one keeps the same number and the windows do nothing.
        let generation = match slot.as_ref() {
            Some(prev)
                if prev.items == items
                    && prev.waits_on_you == waits_on_you
                    && prev.routine_failed == routine_failed =>
            {
                prev.generation
            }
            Some(prev) => prev.generation.wrapping_add(1),
            None => 1,
        };
        *slot = Some(Arc::new(NeedsNow { generation, items, waits_on_you, routine_failed }));
    }

    /// What waits on the user, once per app tick: the order Cmd+J walks and
    /// the two badges on the switcher. Runs before the sidebar early-outs
    /// below, and so whether or not the sidebar is on screen: `routines::
    /// snapshot` only reads on this path, and the jump must not depend on the
    /// view.
    fn sync_needs(&self) {
        let shared = NEEDS_NOW.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let Some(shared) = shared else { return };
        let mut st = self.ivars().sidebar.borrow_mut();
        if st.needs_generation != shared.generation {
            st.needs_generation = shared.generation;
            st.needs = shared.items.clone();
            st.waits_on_you = shared.waits_on_you;
            st.routine_failed = shared.routine_failed;
        }
        if !st.routine_failed {
            st.routine_shown = None;
        }
    }

    /// What the background actions finished since the last tick: the row
    /// stops saying what it is doing, or keeps the failure's words for a
    /// while. Runs before every early-out, so a queued outcome is never
    /// stranded while the sidebar is hidden.
    fn sync_sched_results(&self) {
        let results = crate::routines::drain_results();
        if results.is_empty() {
            return;
        }
        let now = sidebar_asks::now_ms();
        let mut state = self.ivars().sidebar.borrow_mut();
        for result in &results {
            state.sched.finish(result, now);
        }
    }

    /// What the background reopens finished since the last tick: the row
    /// stops saying `Opening…`, or keeps `Could not open` for a while. Runs
    /// before every early-out, like the scheduled actions' outcomes.
    fn sync_team_results(&self) {
        let results = crate::team::drain_results();
        if results.is_empty() {
            return;
        }
        let now = sidebar_asks::now_ms();
        let mut state = self.ivars().sidebar.borrow_mut();
        for result in &results {
            state.agents.finish(result, now);
        }
    }

    /// What the app already knows, in the shape `needs::need_items` takes:
    /// every pane's error and permission timestamps, the routines' last
    /// failure, the open questions. Every window's panes, like
    /// `collect_unread`: the badges and the jump cover the whole app.
    fn need_input(&self, now: u64) -> NeedInput {
        use crate::prompt_preview::PromptPreview;
        let mut panes = Vec::new();
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let app = NSApplication::sharedApplication(mtm);
        let ns_windows = app.windows();
        for i in 0..ns_windows.count() {
            let win = ns_windows.objectAtIndex(i);
            let Some(view) = crate::app::kova_view(&win) else { continue };
            let tabs = view.ivars().tabs.borrow();
            for tab in tabs.iter() {
                tab.for_each_pane(&mut |pane| {
                    // `has_api_error` and `has_permission_prompt` read this
                    // same preview behind a `!is_working` guard: one read and
                    // the same guard, so the model cannot see what the tile
                    // hides.
                    if pane.is_working() {
                        return;
                    }
                    let (error_since, permission_since, detail) =
                        match pane.prompt_preview.borrow().as_ref() {
                            Some(PromptPreview::ApiError { since, kind, text, .. }) => (
                                Some(*since),
                                None,
                                Some(if text.is_empty() { kind.clone() } else { text.clone() }),
                            ),
                            Some(PromptPreview::Permission { since, header, detail, .. }) => {
                                (None, Some(*since), Some(detail.clone().unwrap_or_else(|| header.clone())))
                            }
                            _ => return,
                        };
                    panes.push(PaneNeed {
                        id: u64::from(pane.id),
                        error_since,
                        permission_since,
                        title: pane.display_title("Shell"),
                        // The permission row names who asks: the pane's
                        // agent session, else its own title.
                        from: pane.agent_session_name(),
                        detail,
                    });
                });
            }
        }
        // A run in flight is not a failure: the row says `running`, and the
        // exit code routinectl carries until it ends is the previous one.
        let routines = crate::routines::snapshot()
            .map(|list| {
                list.iter()
                    .filter(|routine| !routine.running)
                    .map(|routine| RoutineNeed {
                        name: routine.name.clone(),
                        failed_at: routine
                            .last
                            .as_ref()
                            .filter(|last| last.code != 0)
                            .and_then(|last| last.at())
                            .map(|when| when.epoch_secs()),
                        paused: routine.paused,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut asks = Vec::new();
        let delegate = crate::app::app_delegate(mtm);
        delegate.ivars().for_each_waiting_ask(|question| {
            asks.push(AskNeed {
                id: question.id.clone(),
                pane_id: question.from.pane_id.map(u64::from),
                created_at: question.asked_at,
                snoozed_until: (question.snoozed_until > 0).then_some(question.snoozed_until),
                // The board only holds what nobody answered.
                answered: false,
                // The row names the asker; the question is its detail.
                from: Some(question.from.name.clone()),
                title: first_line(&question.message),
                detail: Some(rest_of(&question.message)),
            });
        });
        NeedInput { panes, routines, asks, now }
    }

    /// The Asks cards at `now`: every question the board holds and every
    /// permission prompt waiting in a pane, with the tab they came from
    /// (across every window, like `need_input`) and whether a question's
    /// conversation is still alive somewhere.
    fn ask_cards(&self, now: u64) -> Vec<AskCardVm> {
        use crate::prompt_preview::PromptPreview;
        let mut panes: std::collections::HashMap<PaneId, String> = std::collections::HashMap::new();
        let mut permissions = Vec::new();
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let app = NSApplication::sharedApplication(mtm);
        let ns_windows = app.windows();
        for i in 0..ns_windows.count() {
            let win = ns_windows.objectAtIndex(i);
            let Some(view) = crate::app::kova_view(&win) else { continue };
            let tabs = view.ivars().tabs.borrow();
            for tab in tabs.iter() {
                let title = tab.title();
                tab.for_each_pane(&mut |pane| {
                    panes.insert(pane.id, title.clone());
                    if pane.is_working() {
                        return;
                    }
                    // The `since` of the preview and the parsed prompt behind
                    // it are written together (`Pane::refresh_prompt_preview`).
                    let since = match pane.prompt_preview.borrow().as_ref() {
                        Some(PromptPreview::Permission { since, .. }) => *since,
                        _ => return,
                    };
                    let Some(prompt) = pane.permission_prompt() else { return };
                    let from = pane
                        .agent_session_name()
                        .filter(|name| !name.is_empty())
                        .unwrap_or_else(|| pane.display_title("Shell"));
                    permissions.push(AskCardVm::from_permission(prompt, (pane.id, title.clone()), from, since, now));
                });
            }
        }
        let live = crate::claude_session::live_session_ids();
        let mut cards = permissions;
        crate::app::app_delegate(mtm).ivars().for_each_waiting_ask(|question| {
            let pane = question
                .from
                .pane_id
                .and_then(|id| PaneId::try_from(id).ok())
                .and_then(|id| panes.get(&id).map(|tab| (id, tab.clone())));
            let alive = question.from.agent == crate::agent_session::Agent::Claude
                && question
                    .from
                    .session_id
                    .as_deref()
                    .is_some_and(|session| live.iter().any(|id| id == session));
            cards.push(AskCardVm::from_question(question, pane, alive, now));
        });
        cards
    }

    /// Clear what the Asks view only keeps for a moment, and notice a card
    /// the board has just let go of. Runs whether or not the list is up: the
    /// transient card must not outlive a view switch. A permission card lives
    /// as long as its pane's screen shows the prompt, which is the list the
    /// last read left in `asks.cards`.
    fn sync_sidebar_asks_state(&self) {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut waiting = Vec::new();
        crate::app::app_delegate(mtm).ivars().for_each_waiting_ask(|question| {
            waiting.push(sidebar_asks::ask_key(&question.id));
        });
        let mut state = self.ivars().sidebar.borrow_mut();
        waiting.extend(state.asks.cards.iter().filter(|card| card.permission.is_some()).map(|card| card.key));
        state.asks.prune(&waiting, sidebar_asks::now_ms());
    }

    /// Read the tabs into a model and hand it to the sidebar view, which
    /// redraws only when something changed. Also where the list follows a
    /// focus change. Runs once per tick.
    pub(super) fn sync_sidebar(&self) {
        self.sync_sched_results();
        self.sync_team_results();
        self.sync_needs();
        self.sync_sidebar_asks_state();
        let Some(view) = self.ivars().sidebar_view.get() else { return };
        if !self.sidebar_active() {
            return;
        }
        // The unread count across every window, for the Next pill.
        let unread = sidebar::unread_count(&self.collect_unread());
        let (sort, list, waits_on_you, routine_failed, needs, needs_all) = {
            let st = self.ivars().sidebar.borrow();
            (st.sort, st.list, st.waits_on_you, st.routine_failed, st.needs.clone(), st.needs_all)
        };
        let now = now_epoch_secs();
        let facts = self.sidebar_tab_facts();
        // Only the Scheduled list needs the rows; the read itself already
        // happens every tick in `sync_needs`, which is where the badges come
        // from, so this only shapes them.
        let routines = match list {
            SidebarList::Scheduled => {
                let open = self.ivars().sidebar.borrow().open_routine.clone();
                crate::routines::snapshot()
                    .map(|rs| rs.iter().map(|r| RoutineVm::of(r, open.as_deref() == Some(&r.name))).collect())
            }
            SidebarList::Panes | SidebarList::Asks | SidebarList::Agents => None,
        };
        let mut model = SidebarModel::build(&facts, sort, unread, list, routines, &needs);
        model.asks_waiting = waits_on_you;
        model.routine_failed = routine_failed;
        model.apply_needs(&needs, now, needs_all);
        if list == SidebarList::Scheduled {
            // The rows' own state: folding, the row being confirmed, the
            // actions in flight, and the clock the meta row reads.
            let mut st = self.ivars().sidebar.borrow_mut();
            st.sched.now = Some(crate::routines::When::now());
            st.sched.prune(sidebar_asks::now_ms());
            model.sched = st.sched.clone();
            model.sched_meta = sidebar_sched::meta_left(&model.routines);
            model.meta_right = sidebar_sched::clock_words(&st.sched);
        }
        if list == SidebarList::Asks {
            let cards = self.ask_cards(now);
            let mut st = self.ivars().sidebar.borrow_mut();
            st.asks.set_cards(cards);
            // A need row's `Answer` on a permission named its pane: the card
            // for that pane exists only now, so it is this read that lights it.
            if let Some(pane) = st.asks.highlight_pane.take()
                && let Some(card) =
                    st.asks.cards.iter().find(|card| card.permission.is_some() && card.pane_id == Some(pane))
            {
                st.asks.highlight = Some(Highlight {
                    id: card.id.clone(),
                    until_ms: sidebar_asks::now_ms() + sidebar_asks::HIGHLIGHT_MS,
                });
            }
            model.asks = st.asks.clone();
            model.summary = asks_summary(model.asks.open_count());
        }
        if list == SidebarList::Agents {
            // The roster is read only while this list is up; the clock marks
            // panes as they change state, so `Working now` can say how long.
            let roster = crate::team::snapshot();
            let mut st = self.ivars().sidebar.borrow_mut();
            st.agents_clock.observe(&facts, now);
            let agents = sidebar_agents::build(&facts, &needs, &st.agents_clock, now, roster.as_deref());
            st.agents.prune(sidebar_asks::now_ms(), &agents.open_names());
            model.agents = agents;
            model.agent_state = st.agents.clone();
            model.summary = vec![(sidebar_agents::summary_words(&model.agents), sidebar::SummaryRun::Dot)];
            model.meta_right = None;
        }
        view.set_model(model);

        // A need row that walked to an ask scrolls its card into view, once.
        let reveal = {
            let mut st = self.ivars().sidebar.borrow_mut();
            let highlighted = st.asks.highlight.as_ref().map(|h| h.id.clone());
            match highlighted {
                Some(id) if st.last_revealed_ask.as_deref() != Some(id.as_str()) => {
                    st.last_revealed_ask = Some(id.clone());
                    Some(id)
                }
                Some(_) => None,
                None => {
                    st.last_revealed_ask = None;
                    None
                }
            }
        };
        if let Some(id) = reveal {
            view.reveal_ask(&id);
        }

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
    pub(super) fn sidebar_tab_facts(&self) -> Vec<TabFacts> {
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

    /// The view switcher: show `list`. Clicking the selected icon is a no-op,
    /// like a segmented control.
    pub(super) fn sidebar_show_list(&self, list: SidebarList) {
        let mut st = self.ivars().sidebar.borrow_mut();
        st.list = list;
        if list != SidebarList::Asks {
            // The field and the overlay belong to the Asks list; the draft
            // stays in `asks.drafts` for when the user comes back. A pending
            // permission highlight would name a card nobody can see.
            st.asks.brief = None;
            st.asks.highlight_pane = None;
        }
    }

    /// Click a routine: open it, or close it when it is the open one.
    pub(super) fn sidebar_toggle_routine(&self, name: &str) {
        let mut st = self.ivars().sidebar.borrow_mut();
        st.open_routine = match st.open_routine.as_deref() {
            Some(open) if open == name => None,
            _ => Some(name.to_string()),
        };
    }

    /// A click on a section header of the Scheduled list: fold or unfold it.
    pub(super) fn sidebar_sched_section(&self, group: crate::routines::Group) {
        self.ivars().sidebar.borrow_mut().sched.toggle(group);
    }

    /// A click on a Scheduled row or one of its buttons. `log` is the path
    /// routinectl reported for the routine; `Open log` falls back to
    /// `logs/<name>.log` when it reported none.
    pub(super) fn sidebar_sched_hit(&self, hit: SchedHit, name: &str, log: &str) {
        let mut run = None;
        let mut open_log = false;
        let mut edit = false;
        {
            let mut st = self.ivars().sidebar.borrow_mut();
            let SchedHit::Button(_, button) = hit else {
                // The row itself opens its body. A section never reaches here:
                // the view sends that one to `sidebar_sched_section`.
                st.sched.close_confirm();
                drop(st);
                if let SchedHit::Row(_) = hit {
                    self.sidebar_toggle_routine(name);
                }
                return;
            };
            // An action in flight owns the row: its buttons do nothing until
            // it reports back.
            if st.sched.busy_action(name).is_some() {
                return;
            }
            match button {
                SchedButton::Run | SchedButton::Pause | SchedButton::Resume | SchedButton::ConfirmDelete => {
                    let action = button.action().expect("a button that runs a verb");
                    st.sched.start(name, action);
                    st.sched.close_confirm();
                    run = Some(action);
                }
                // Deleting asks first, inline.
                SchedButton::Delete => st.sched.open_confirm(name),
                SchedButton::ConfirmCancel => st.sched.close_confirm(),
                SchedButton::Log => open_log = true,
                SchedButton::Edit => edit = true,
            }
        }
        if let Some(action) = run {
            crate::routines::run_action(action, name);
            return;
        }
        let opened = if open_log {
            let path = if log.is_empty() { crate::routines::log_path(name).display().to_string() } else { log.to_string() };
            crate::routines::open_log(&path).map_err(|why| format!("Could not open: {why}"))
        } else if edit {
            let path = crate::routines::prompt_path(name).display().to_string();
            crate::routines::edit_prompt(&path).map_err(|why| format!("Could not open: {why}"))
        } else {
            return;
        };
        if let Err(words) = opened {
            self.ivars().sidebar.borrow_mut().sched.failed(name, words, sidebar_asks::now_ms());
        }
    }

    /// Escape or `Cancel` on the inline delete confirmation.
    pub(super) fn sidebar_sched_cancel_confirm(&self) {
        self.ivars().sidebar.borrow_mut().sched.close_confirm();
    }

    /// `Show all` / `Show less` on the `Needs you` block.
    pub(super) fn sidebar_toggle_needs_all(&self) {
        let mut st = self.ivars().sidebar.borrow_mut();
        st.needs_all = !st.needs_all;
    }

    /// A stat tile of the Agents list: its section replaces the one under it.
    pub(super) fn sidebar_agents_filter(&self, section: sidebar_agents::AgentSection) {
        self.ivars().sidebar.borrow_mut().agents.set_filter(section);
    }

    /// A `By tab` row: switch to the Panes list on that tab.
    pub(super) fn sidebar_agents_tab(&self, tab_idx: usize) {
        self.ivars().sidebar.borrow_mut().list = SidebarList::Panes;
        self.do_switch_tab(tab_idx);
    }

    /// The folded `Closed N` of the dedicated sessions.
    pub(super) fn sidebar_agents_toggle_closed(&self) {
        self.ivars().sidebar.borrow_mut().agents.toggle_closed();
    }

    /// The `Open` button of a closed roster entry: validate its argv, say
    /// `Opening…`, and let `crate::team` run it on a background thread.
    pub(super) fn sidebar_agents_open(&self, name: &str) {
        let now = sidebar_asks::now_ms();
        if self.ivars().sidebar.borrow().agents.busy(name) {
            return;
        }
        let roster = crate::team::snapshot();
        let Some(entry) = roster.as_deref().and_then(|r| r.entry(name)) else { return };
        let argv = entry.reopen.clone();
        if let Err(why) = crate::team::reopen(name, &argv) {
            log::warn!("agents: {why}");
            self.ivars().sidebar.borrow_mut().agents.failed(name, now);
            return;
        }
        self.ivars().sidebar.borrow_mut().agents.start(name, now);
    }

    /// A row of the `Needs you` block, by its stable key: an error focuses
    /// its pane, a failed routine opens it in the Scheduled list, an ask or
    /// a permission opens the Asks list on its card.
    pub(super) fn sidebar_open_need(&self, key: &str) {
        let item = {
            let st = self.ivars().sidebar.borrow();
            st.needs.iter().find(|item| item.key == key).cloned()
        };
        let Some(item) = item else { return };
        match item.kind {
            NeedKind::Error => {
                if let Some(pane_id) = item.pane_id.and_then(|id| PaneId::try_from(id).ok()) {
                    focus_pane_anywhere(pane_id);
                }
            }
            NeedKind::FailedRoutine => {
                let mut st = self.ivars().sidebar.borrow_mut();
                st.list = SidebarList::Scheduled;
                st.open_routine = Some(item.title);
            }
            NeedKind::Ask | NeedKind::Permission => {
                let mut st = self.ivars().sidebar.borrow_mut();
                st.list = SidebarList::Asks;
                st.asks.highlight = match item.kind {
                    NeedKind::Ask => item.key.strip_prefix("ask:").map(|id| Highlight {
                        id: id.to_string(),
                        until_ms: sidebar_asks::now_ms() + sidebar_asks::HIGHLIGHT_MS,
                    }),
                    _ => None,
                };
                // A permission card's id carries the screen's fingerprint, which
                // the list only learns on its next read: name the pane now, the
                // card is lit there (`sync_sidebar`).
                st.asks.highlight_pane = if item.kind == NeedKind::Permission {
                    item.pane_id.and_then(|id| PaneId::try_from(id).ok())
                } else {
                    None
                };
                st.last_revealed_ask = None;
            }
        }
    }

    /// A click on an Asks card, or in the Brief overlay: run the state
    /// machine, queue the answer, and land the view where the button says.
    /// A permission button is the one click that types into a pane: it reads
    /// the screen again first and gives up when it moved on.
    pub(super) fn sidebar_ask_hit(&self, hit: AsksHit) {
        if let Some((key, choice)) = hit.permission() {
            self.sidebar_permission_hit(key, choice);
            return;
        }
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let now = sidebar_asks::now_ms();
        let (action, asks) = {
            let mut st = self.ivars().sidebar.borrow_mut();
            let action = sidebar_asks::click(&mut st.asks, hit, now);
            (action, st.asks.clone())
        };
        if let Some(view) = self.ivars().sidebar_view.get() {
            // The state machine lives here, the field and the overlay live in
            // the view: hand it the result now, not on the next tick.
            view.adopt_asks(asks);
        }
        if let Some(action) = action {
            crate::app::app_delegate(mtm).ivars().queue_ask_action(action);
        }
        if let AsksHit::Open(pane_id) = hit {
            focus_pane_anywhere(pane_id);
        }
        if let AsksHit::OpenDoc(key) = hit {
            let open = self.ivars().sidebar.borrow().asks.card(key).and_then(|card| Some((card.id.clone(), card.decision_url.clone()?)));
            if let Some((id, url)) = open {
                crate::ask_card::open_in_panel(id, url);
            }
        }
    }

    /// Answer a permission prompt from its card: the pane reads its screen
    /// again and types the digit only while it is still the prompt the card
    /// shows. On a match the card goes to `Sending`; otherwise it says
    /// `Prompt changed, open the pane` in crit and the pane keeps its new
    /// preview, which the next read of the list shows.
    fn sidebar_permission_hit(&self, key: u64, choice: crate::prompt_preview::PermissionChoice) {
        let typed = {
            let st = self.ivars().sidebar.borrow();
            let Some(card) = st.asks.card(key) else { return };
            let (Some(permission), Some(pane_id)) = (card.permission.as_ref(), card.pane_id) else { return };
            with_pane(pane_id, |pane| pane.answer_permission(&permission.prompt, choice)).unwrap_or(false)
        };
        let now = sidebar_asks::now_ms();
        let asks = {
            let mut st = self.ivars().sidebar.borrow_mut();
            if typed {
                sidebar_asks::permission_sending(&mut st.asks, key, now);
            } else if let Some(pane_id) = st.asks.card(key).and_then(|card| card.pane_id) {
                sidebar_asks::permission_refused(&mut st.asks, pane_id, now);
            }
            st.asks.clone()
        };
        if let Some(view) = self.ivars().sidebar_view.get() {
            view.adopt_asks(asks);
        }
    }

    /// The Brief field's live text: kept in the state so reopening the
    /// overlay on the same ask restores it. The view's own model copy is
    /// updated on the same keystroke, for the card's height.
    pub(super) fn sidebar_brief_draft(&self, id: &str, text: &str) {
        let mut st = self.ivars().sidebar.borrow_mut();
        if text.is_empty() {
            st.asks.drafts.remove(id);
        } else {
            st.asks.drafts.insert(id.to_string(), text.to_string());
        }
    }

    /// The Next pill: `Nothing to read` is not a button; a pill with a stop
    /// (a need item with a pane, else unread output) is, and does what
    /// Cmd+J does.
    pub(super) fn sidebar_next_pill_clicked(&self) {
        let unread = sidebar::unread_count(&self.collect_unread());
        let clickable = next_pill(&self.ivars().sidebar.borrow().needs, unread).clickable();
        if clickable {
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
    fn pane_snapshot(&self, pane_id: PaneId) -> Option<(String, bool, bool, bool, bool, bool, bool, bool)> {
        let tabs = self.ivars().tabs.borrow();
        let bookmark_keys = self.ivars().bookmark_keys.borrow();
        let active_idx = self.ivars().active_tab.get();
        for (ti, tab) in tabs.iter().enumerate() {
            if let Some(pane) = tab.pane(pane_id) {
                let bookmarked = match pane.agent_session_id() {
                    Some(id) => bookmark_keys.contains(&id),
                    None => pane.cwd().is_some_and(|cwd| bookmark_keys.contains(&cwd)),
                };
                let seen = ti == active_idx && tab.focused_pane == pane_id;
                return Some((
                    pane.display_title("shell"),
                    pane.is_working(),
                    pane.has_permission_prompt(),
                    pane.is_bare_shell(),
                    pane.restored_session().is_some(),
                    pane.minimized,
                    bookmarked,
                    pane_flags(pane, seen).is_unread(),
                ));
            }
        }
        None
    }

    /// Cmd+U on a tile or on the focused pane: an unread pane becomes read
    /// (every seen flag set, the manual mark dropped), a read one is marked
    /// unread by hand. The mark survives while the pane stays focused; the
    /// next time the pane becomes focused it is dropped (`tick.rs`).
    pub(super) fn toggle_pane_unread(&self, pane_id: PaneId) {
        let Some((.., unread)) = self.pane_snapshot(pane_id) else { return };
        let tabs = self.ivars().tabs.borrow();
        let Some(pane) = tabs.iter().find_map(|t| t.pane(pane_id)) else { return };
        if unread {
            pane.mark_read();
        } else {
            pane.set_manual_unread(true);
        }
        drop(tabs);
        self.mark_dirty();
    }

    /// Cmd+U: `toggle_pane_unread` on the focused pane of the active tab.
    pub(super) fn do_toggle_unread(&self) {
        let focused = {
            let tabs = self.ivars().tabs.borrow();
            tabs.get(self.ivars().active_tab.get()).map(|t| t.focused_pane)
        };
        if let Some(id) = focused {
            self.toggle_pane_unread(id);
        }
    }

    /// Whether the focused pane of the active tab is unread (the Pane menu's
    /// `Mark as Read` / `Mark as Unread` title).
    pub(super) fn focused_pane_unread(&self) -> bool {
        let focused = {
            let tabs = self.ivars().tabs.borrow();
            tabs.get(self.ivars().active_tab.get()).map(|t| t.focused_pane)
        };
        focused.and_then(|id| self.pane_snapshot(id)).is_some_and(|(.., unread)| unread)
    }

    /// The tile's context menu, KovaLink's swipes and sheets as items,
    /// popped up at `location` in `view`.
    pub(super) fn show_sidebar_pane_menu(&self, view: &objc2_app_kit::NSView, location: CGPoint, pane_id: PaneId) {
        let Some((_, working, awaiting, bare, resumable, minimized, bookmarked, unread)) = self.pane_snapshot(pane_id) else { return };
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
        rows.push(MenuRow::Item(if unread { "Mark read" } else { "Mark unread" }.into(), PaneAction::ToggleUnread.tag()));
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
        let mut rows = color_rows(self.ivars().tabs.borrow().get(tab_idx).and_then(|t| t.color));
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
        let rows = color_rows(current);
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
                    item.setState(objc2_app_kit::NSControlStateValueOn);
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
            PaneAction::ToggleUnread => self.toggle_pane_unread(pane_id),
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

    /// Run the resume line a restored pane holds (`Pane::run_resume`, shared
    /// with the IPC `resume-pane`). Refused, like `Start Claude`, when something
    /// already runs there.
    fn resume_in_pane(&self, pane_id: PaneId) {
        if self.ipc_resume_pane(pane_id) == Some(false) {
            self.set_transient_status("Nothing to resume in this pane");
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
#[cfg(test)]
fn split_layout(bounds: CGRect, width: f64, min_split: f64, wanted: bool) -> (bool, CGRect, CGRect) {
    let (show, sidebar, kova, _) = split_frames(bounds, width, min_split, wanted, false, 0.5);
    (show, sidebar, kova)
}

/// `split_layout` with the side panel at the right: the frames of the
/// sidebar, the Metal view and the panel (when shown), from
/// `side_panel::split`. Without a panel it is the sidebar's split exactly.
fn split_frames(
    bounds: CGRect,
    width: f64,
    min_split: f64,
    wanted: bool,
    panel_wanted: bool,
    fraction: f32,
) -> (bool, CGRect, CGRect, Option<CGRect>) {
    let h = bounds.size.height;
    let s = super::side_panel::split(bounds.size.width, width, wanted, panel_wanted, fraction, min_split);
    let rect = |x: f64, w: f64| CGRect { origin: CGPoint { x, y: 0.0 }, size: CGSize { width: w, height: h } };
    let sidebar_frame = rect(0.0, width);
    let my_frame = if s.sidebar || s.panel.is_some() { rect(s.term_x, s.term_w) } else { bounds };
    (s.sidebar, sidebar_frame, my_frame, s.panel.map(|(x, w)| rect(x, w)))
}

/// One tab's width override as the side panel opens, resizes or closes:
/// `(override, pin)`, the pin being the override Kova set and must undo later.
/// `now` is the tab's override, `ours` the pin Kova set last time, `wide_vw`
/// the tab's width without the panel (logical points, like `term_w`).
fn panel_pin(now: f32, ours: Option<f32>, wide_vw: f32, term_w: f32, scale: f32, open: bool) -> (f32, Option<f32>) {
    let mine = ours.is_some_and(|v| (now - v).abs() < 0.5);
    if now > 0.0 && !mine {
        return (now, None);
    }
    let unpinned = if mine { 0.0 } else { now };
    if !open {
        return (unpinned, None);
    }
    match crate::pane::pinned_virtual_width(wide_vw, term_w, scale) {
        Some(px) => (px, Some(px)),
        None => (unpinned, None),
    }
}

/// The room right of the sidebar in the window `view` sits in: what the
/// panel's fraction is a share of.
pub(super) fn room_right_of_sidebar(view: &objc2_app_kit::NSView) -> f64 {
    let Some(win) = view.window() else { return 0.0 };
    let Some(kova) = crate::app::kova_view(&win) else { return 0.0 };
    let Some(container) = (unsafe { kova.superview() }) else { return 0.0 };
    let w = container.bounds().size.width;
    if kova.ivars().sidebar_shown.get() { w - sidebar::width_pt() as f64 } else { w }
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

/// The first non-empty line of a question: the need row's title when the
/// asker did not name itself. A markdown body would otherwise push
/// everything else out.
fn first_line(text: &str) -> String {
    text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default().to_string()
}

/// What a question says after its first line, on one line: the row's detail
/// under the asker's name. Empty when the question is one line.
fn rest_of(text: &str) -> String {
    let mut rest = text.lines().map(str::trim).filter(|line| !line.is_empty());
    rest.next();
    rest.collect::<Vec<_>>().join(" ")
}

/// Focus a pane wherever it lives. A need item belongs to the app, not to
/// one window (the sidebar lists every window's errors), so the click brings
/// the window that holds the pane forward and lets it switch to the tab.
/// Like `attention::focus_pane_in_any_window` without the big directory
/// overlay: a click in the sidebar already says where it goes.
fn focus_pane_anywhere(pane_id: PaneId) {
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let app = NSApplication::sharedApplication(mtm);
    let ns_windows = app.windows();
    for i in 0..ns_windows.count() {
        let win = ns_windows.objectAtIndex(i);
        let Some(view) = crate::app::kova_view(&win) else { continue };
        if view.ipc_focus_pane(pane_id) {
            win.makeKeyAndOrderFront(None);
            return;
        }
    }
    log::debug!("focus_pane_anywhere: pane {} not found", pane_id);
}

/// Run `f` on the pane with `id`, in whatever window holds it. A permission
/// card can belong to any window's pane (the list shows them all), and the
/// click has to reach the pane itself, not just the window.
pub(super) fn with_pane<R>(pane_id: PaneId, f: impl FnOnce(&Pane) -> R) -> Option<R> {
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let app = NSApplication::sharedApplication(mtm);
    let ns_windows = app.windows();
    let mut f = Some(f);
    for i in 0..ns_windows.count() {
        let win = ns_windows.objectAtIndex(i);
        let Some(view) = crate::app::kova_view(&win) else { continue };
        let tabs = view.ivars().tabs.borrow();
        let mut out = None;
        for tab in tabs.iter() {
            tab.for_each_pane(&mut |pane| {
                if out.is_none()
                    && pane.id == pane_id
                    && let Some(f) = f.take()
                {
                    out = Some(f(pane));
                }
            });
        }
        if out.is_some() {
            return out;
        }
    }
    None
}

/// The flags a pane's tile state is derived from. `seen` is true for the
/// focused pane of the active tab, whose unread marks are being looked at.
pub(super) fn pane_flags(pane: &Pane, seen: bool) -> PaneFlags {
    let (bell, completion) = {
        let term = pane.terminal.read();
        (term.bell.load(std::sync::atomic::Ordering::Relaxed), term.unread_completion())
    };
    PaneFlags {
        permission_prompt: pane.has_permission_prompt(),
        prompt_unseen: pane.is_prompt_unseen(),
        bell,
        completion,
        hook_unseen: pane.is_awaiting_unseen(),
        turn_end_unseen: pane.is_turn_end_unseen(),
        manual: pane.is_manual_unread(),
        working: pane.is_working(),
        starting: pane.is_starting_agent(),
        idle_agent: pane.is_idle_agent(),
        error: pane.has_api_error(),
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
    fn the_panel_pins_the_width_the_tab_had_without_it() {
        // 1600 pt of terminal, the panel takes 800: the tab keeps 1600 pt (3200 px at 2x).
        assert_eq!(panel_pin(0.0, None, 1600.0, 800.0, 2.0, true), (3200.0, Some(3200.0)));
        // Dragging the panel: the pin follows the width without the panel.
        assert_eq!(panel_pin(3200.0, Some(3200.0), 1700.0, 700.0, 2.0, true), (3400.0, Some(3400.0)));
        // Closing it takes Kova's pin off.
        assert_eq!(panel_pin(3400.0, Some(3400.0), 1700.0, 1700.0, 2.0, false), (0.0, None));
    }

    #[test]
    fn the_panel_leaves_a_width_the_user_set_alone() {
        assert_eq!(panel_pin(5000.0, None, 1600.0, 800.0, 2.0, true), (5000.0, None));
        // Pinned by Kova, then changed by the user (a split, a resize): theirs now.
        assert_eq!(panel_pin(4000.0, Some(3200.0), 1600.0, 800.0, 2.0, false), (4000.0, None));
    }

    #[test]
    fn the_colour_menu_names_every_tab_colour_and_round_trips_its_tags() {
        assert_eq!(TAB_COLOR_NAMES.len(), TAB_COLORS.len());
        assert_eq!(TAB_COLORS.len(), 12);
        for i in 0..TAB_COLOR_NAMES.len() {
            assert_eq!(TabAction::from_tag(TabAction::Color(i).tag()), Some(TabAction::Color(i)));
        }
        assert_eq!(TabAction::from_tag(TAB_COLORS.len() as isize), None);
        for a in [TabAction::NoColor, TabAction::Rename, TabAction::AddPane, TabAction::CollapseOthers, TabAction::Close] {
            assert_eq!(TabAction::from_tag(a.tag()), Some(a));
            assert!(a.tag() >= TAB_COLORS.len() as isize);
        }
        assert!(TAB_COLOR_NAMES.iter().all(|n| n.is_ascii()));
        // The first six keep their indices: saved sessions store them.
        assert_eq!(&TAB_COLOR_NAMES[..6], &["Red", "Orange", "Yellow", "Green", "Blue", "Violet"]);
        let rows = color_rows(Some(7));
        assert_eq!(rows.len(), 13);
        assert!(matches!(&rows[7], MenuRow::Swatch(n, 7, Some(_), true) if n == "Coral"));
        assert!(matches!(&rows[12], MenuRow::Swatch(n, TAG_NO_COLOR, None, false) if n == "No colour"));
        assert!(matches!(&color_rows(None)[12], MenuRow::Swatch(_, _, None, true)));
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
