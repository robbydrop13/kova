//! The Agents view of the sidebar: what every pane is doing (working, waiting
//! on you, idle, failed), the counts per tab, and the team roster the panes
//! belong to. Pure: `build` turns the window's pane facts into the view model,
//! `layout` turns that model plus the view state into rects, `hit` turns a
//! point into an `AgentsHit`, `draw` paints one row.
//!
//! `sidebar_ui` runs the clicks and owns the state between ticks; `crate::team`
//! reads the roster and reopens a session on a background thread. See
//! `docs/sidebar-spec.md`.

use std::collections::HashMap;

use super::feather::Icon;
use super::needs::{NeedItem, NeedKind};
use super::sidebar::{format_age, tab_tint, tokens};
use super::sidebar_model::{header_title, PaneFacts, TabFacts, TileVm};
use super::sidebar_view::{
    draw_text, fill_dot, fill_round, stroke_icon, stroke_ring, stroke_round, Align, AppKitMetrics, ListHit, Rect, Row,
    RowKind, Shared, Style, TextMetrics, EDGE_TOLERANCE, LIST_BOTTOM, LIST_TOP, PAD_H,
};
use crate::pane::PaneId;
use crate::team::{self, State as RosterState};

// ---------------------------------------------------------------
// Metrics (points)
// ---------------------------------------------------------------

/// One stat card: 8 pt of padding, the 20 pt value, then the label line.
pub const STAT_H: f64 = 56.0;
const STAT_GAP: f64 = 6.0;
const STAT_RADIUS: f64 = 8.0;
const STAT_PAD_X: f64 = 10.0;
const STAT_PAD_Y: f64 = 8.0;
const STAT_VALUE_H: f64 = 24.0;
const STAT_LABEL_H: f64 = 16.0;
const STAT_DOT: f64 = 6.0;
/// Four tiles in a row from this width up, two by two below it.
pub const STAT_WIDE: f64 = 360.0;

/// A section header: its row, the label, the count after it, the note at the
/// right, and the chevron of the folded `Closed N`.
pub const SECTION_H: f64 = 24.0;
pub const SECTION_GAP: f64 = 8.0;
const SECTION_PAD_X: f64 = 6.0;
const SECTION_CHEVRON_X: f64 = 6.0;
const SECTION_LABEL_X: f64 = 6.0;
const SECTION_FOLDED_X: f64 = 24.0;
const SECTION_COUNT_GAP: f64 = 6.0;
const CHEVRON: f64 = 12.0;

/// A pane, team or dedicated row: two 15 pt lines, 5 above and under them.
pub const ROW_H: f64 = 40.0;
const ROW_GAP: f64 = 2.0;
const ROW_PAD_X: f64 = 6.0;
const ROW_PAD_Y: f64 = 5.0;
const ROW_RADIUS: f64 = 6.0;
const LINE_H: f64 = 15.0;
const TEXT_X: f64 = 24.0;
const TEXT_GAP: f64 = 6.0;
const DOT: f64 = 8.0;
const DOT_BOX: f64 = 16.0;
pub const EMPTY_H: f64 = 22.0;

/// A `By tab` row: the tab's colour dot, its name, and four 22 pt count
/// columns against the right edge.
pub const TAB_ROW_H: f64 = 26.0;
/// The tab's colour dot, the marker the group headers carry too.
const TAB_DOT_D: f64 = 8.0;
const TAB_TEXT_X: f64 = 18.0;
const TAB_COL_W: f64 = 22.0;
const TAB_END_PAD: f64 = 6.0;

/// The `Open` button of a closed roster entry.
pub const OPEN_H: f64 = 22.0;
const OPEN_PAD: f64 = 8.0;
const OPEN_RADIUS: f64 = 6.0;
const OPEN_LABEL: &str = "Open";
const OPENING: &str = "Opening\u{2026}";
const FAILED_OPEN: &str = "Could not open";

/// How long a reopen in flight keeps the button quiet before the next click
/// is allowed again.
pub const OPEN_TIMEOUT_MS: u64 = 20_000;
/// How long a failed reopen's words stay on the row.
pub const ERROR_MS: u64 = 10_000;

// ---------------------------------------------------------------
// The four states
// ---------------------------------------------------------------

/// What a pane is doing, in the order the stat tiles read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgentSection {
    Working,
    Need,
    Idle,
    Error,
}

impl AgentSection {
    pub const ALL: [AgentSection; 4] = [Self::Working, Self::Need, Self::Idle, Self::Error];

    /// The stat tile's label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Working => "Working",
            Self::Need => "Need you",
            Self::Idle => "Idle",
            Self::Error => "Error",
        }
    }

    /// The section's title.
    pub fn title(self) -> &'static str {
        match self {
            Self::Working => "Working now",
            Self::Need => "Need you",
            Self::Idle => "Idle",
            Self::Error => "Error",
        }
    }

    /// The note at the section's right.
    pub fn note(self) -> Option<&'static str> {
        match self {
            Self::Working => Some("longest first"),
            _ => None,
        }
    }

    /// The dot's fill; `Idle` is a ring, drawn by its caller.
    pub fn color(self) -> [f32; 3] {
        match self {
            Self::Working => tokens::get().working,
            Self::Need => tokens::get().awaiting,
            Self::Error => tokens::get().error,
            Self::Idle => tokens::get().text_tertiary,
        }
    }

    /// The ink of the state word and of the count columns.
    pub fn ink(self) -> [f32; 3] {
        match self {
            Self::Working => tokens::get().info_text,
            Self::Need => tokens::get().warn_text,
            Self::Error => tokens::get().crit_text,
            Self::Idle => tokens::get().text_tertiary,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Working => 0,
            Self::Need => 1,
            Self::Idle => 2,
            Self::Error => 3,
        }
    }
}

/// The columns of a `By tab` row, left to right: need you, working, error,
/// idle, the same four dots as the legend.
const TAB_COLUMNS: [AgentSection; 4] = [AgentSection::Need, AgentSection::Working, AgentSection::Error, AgentSection::Idle];

/// Which section a header titles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionKind {
    /// The state the stat tiles filter on.
    Filter(AgentSection),
    ByTab,
    Team,
    Dedicated,
    /// The folded `Closed N` under the dedicated sessions.
    Closed,
}

impl SectionKind {
    fn label(self) -> &'static str {
        match self {
            SectionKind::Filter(s) => s.title(),
            SectionKind::ByTab => "By tab",
            SectionKind::Team => "Team",
            SectionKind::Dedicated => "Dedicated sessions",
            SectionKind::Closed => "Closed",
        }
    }

    fn note(self) -> Option<&'static str> {
        match self {
            SectionKind::Filter(s) => s.note(),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------
// View model
// ---------------------------------------------------------------

/// One pane as the Agents view lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentPane {
    pub pane_id: PaneId,
    pub title: String,
    /// The tab it lives in, shown on line 2.
    pub tab: String,
    /// The rest of line 2.
    pub detail: String,
    pub section: AgentSection,
    /// The right column: an age for working and idle, the age of the wait
    /// for need you, `error` for a failed turn.
    pub meta: String,
    /// The age in seconds, for the `longest first` order.
    pub age_secs: Option<u64>,
    /// Epoch seconds the wait began, for `oldest first`.
    pub since: Option<u64>,
    /// The need item's stable key (`ask:<id>`, `pane:12`): what a click opens.
    pub key: Option<String>,
}

/// One tab, with the counts its `By tab` row shows.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentTab {
    pub tab_idx: usize,
    pub title: String,
    pub color: Option<usize>,
    /// Indexed by `AgentSection::index()`.
    pub counts: [usize; 4],
}

impl AgentTab {
    pub fn count(&self, section: AgentSection) -> usize {
        self.counts[section.index()]
    }
}

/// One roster entry, matched to the pane that runs it when there is one.
#[derive(Clone, Debug, PartialEq)]
pub struct TeamRow {
    pub name: String,
    pub role: String,
    pub project: Option<String>,
    /// Open in the roster.
    pub open: bool,
    /// The pane running it, when it is open and this window holds it.
    pub pane_id: Option<PaneId>,
    /// The right column of an open row: the pane's age or state word.
    pub meta: String,
    pub pane_state: Option<AgentSection>,
    /// The argv the CLI reports; `crate::team::reopen` validates it again.
    pub reopen: Vec<String>,
}

/// Everything the Agents list shows for one window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentsVm {
    /// Every pane of the window, in tab and column order.
    pub panes: Vec<AgentPane>,
    pub tabs: Vec<AgentTab>,
    pub team: Vec<TeamRow>,
    pub dedicated: Vec<TeamRow>,
    /// `waits_on_you`: asks and permissions across the app, the stat tile's
    /// number. A row only appears for a pane of this window.
    pub need_count: usize,
    /// False until the first roster read lands.
    pub team_loaded: bool,
    /// The roster CLI is missing or unreadable: both team sections hide.
    pub team_available: bool,
    /// Panes in the window and the tabs they fill, for the meta row.
    pub sessions: usize,
    pub tab_count: usize,
}

impl AgentsVm {
    pub fn stat(&self, section: AgentSection) -> usize {
        match section {
            AgentSection::Need => self.need_count,
            _ => self.panes.iter().filter(|p| p.section == section).count(),
        }
    }

    /// Every roster name the CLI reports open: what clears a busy button.
    pub fn open_names(&self) -> Vec<String> {
        self.team.iter().chain(self.dedicated.iter()).filter(|r| r.open).map(|r| r.name.clone()).collect()
    }
}

/// `N sessions in M tabs`, the Agents list's meta row.
pub fn summary_words(vm: &AgentsVm) -> String {
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    format!("{} session{} in {} tab{}", vm.sessions, plural(vm.sessions), vm.tab_count, plural(vm.tab_count))
}

// ---------------------------------------------------------------
// Working and idle ages
// ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Working,
    Idle,
}

/// When each pane entered the state it is in now. Pane facts carry no
/// timestamp, so the view keeps its own clock: the first tick that sees a
/// pane working marks it, and the mark moves when it goes idle and back.
/// Ticks only run while the Agents view is up, so an age means "since this
/// view last watched it", not since the process started.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentsClock {
    marks: Vec<(PaneId, Activity, u64)>,
}

impl AgentsClock {
    /// Move every mark to the pane's current activity, and drop the panes
    /// that left the window.
    pub fn observe(&mut self, tabs: &[TabFacts], now: u64) {
        let mut next = Vec::with_capacity(self.marks.len());
        for tab in tabs {
            for p in &tab.panes {
                let activity = if p.flags.working || p.flags.starting { Activity::Working } else { Activity::Idle };
                let since = self
                    .marks
                    .iter()
                    .find(|(id, a, _)| *id == p.pane_id && *a == activity)
                    .map_or(now, |(_, _, s)| *s);
                next.push((p.pane_id, activity, since));
            }
        }
        self.marks = next;
    }

    pub fn age(&self, pane: PaneId, activity: Activity, now: u64) -> Option<u64> {
        self.marks.iter().find(|(id, a, _)| *id == pane && *a == activity).map(|(_, _, since)| now.saturating_sub(*since))
    }
}

fn clock_activity(p: &PaneFacts) -> Activity {
    if p.flags.working || p.flags.starting { Activity::Working } else { Activity::Idle }
}

// ---------------------------------------------------------------
// Building the model
// ---------------------------------------------------------------

/// The name a tab shows: its own title, else the header name of its focused
/// pane (what the group header shows).
fn tab_name(tab: &TabFacts) -> String {
    let title = tab.title.trim();
    if !title.is_empty() {
        return title.to_string();
    }
    tab.panes.iter().find(|p| p.focused).or_else(|| tab.panes.first()).map(header_title).unwrap_or_else(|| "shell".to_string())
}

/// Classify every pane, count the tabs and match the roster to the panes.
/// `items` is the tick's `needs::need_items`: asks and permissions are what
/// `Need you` lists, and an ask's age is its `since`.
pub fn build(tabs: &[TabFacts], items: &[NeedItem], clock: &AgentsClock, now: u64, roster: Option<&team::Team>) -> AgentsVm {
    let mut need_of: HashMap<PaneId, &NeedItem> = HashMap::new();
    for item in items.iter().filter(|i| matches!(i.kind, NeedKind::Ask | NeedKind::Permission)) {
        if let Some(id) = item.pane_id.and_then(|id| PaneId::try_from(id).ok()) {
            need_of.entry(id).or_insert(item);
        }
    }
    let need_count = items.iter().filter(|i| matches!(i.kind, NeedKind::Ask | NeedKind::Permission)).count();

    let mut panes = Vec::new();
    let mut tabs_vm: Vec<AgentTab> = tabs
        .iter()
        .map(|t| AgentTab { tab_idx: t.tab_idx, title: tab_name(t), color: t.color, counts: [0; 4] })
        .collect();
    let mut sessions = 0;
    for (ti, tab) in tabs.iter().enumerate() {
        for p in &tab.panes {
            sessions += 1;
            let tile = TileVm::from_facts(p);
            let need = need_of.get(&p.pane_id);
            let section = if need.is_some() {
                AgentSection::Need
            } else if p.flags.error && !p.flags.working {
                AgentSection::Error
            } else if p.flags.working || p.flags.starting {
                AgentSection::Working
            } else {
                AgentSection::Idle
            };
            let age_secs = clock.age(p.pane_id, clock_activity(p), now);
            let (title, detail, meta, since, key) = match (section, need) {
                (AgentSection::Need, Some(item)) => {
                    let title = if item.title.trim().is_empty() { tile.title.clone() } else { item.title.clone() };
                    let detail = if item.detail.trim().is_empty() { tile.tooltip.clone() } else { item.detail.clone() };
                    (title, detail, format_age(now.saturating_sub(item.since)), Some(item.since), Some(item.key.clone()))
                }
                (AgentSection::Error, _) => (tile.title.clone(), tile.tooltip.clone(), "error".to_string(), None, None),
                (AgentSection::Working, _) => {
                    let meta = age_secs.map(format_age).unwrap_or_else(|| section.label().to_lowercase());
                    (tile.title.clone(), tile.tooltip.clone(), meta, None, None)
                }
                _ => {
                    let meta = age_secs.map(format_age).unwrap_or_default();
                    (tile.title.clone(), tile.tooltip.clone(), meta, None, None)
                }
            };
            if let Some(t) = tabs_vm.get_mut(ti) {
                t.counts[section.index()] += 1;
            }
            panes.push(AgentPane {
                pane_id: p.pane_id,
                title,
                tab: tab_name(tab),
                detail,
                section,
                meta,
                age_secs,
                since,
                key,
            });
        }
    }

    let mut vm = AgentsVm {
        panes,
        tabs: tabs_vm,
        need_count,
        team_loaded: roster.is_some(),
        team_available: roster.is_some_and(|r| r.available),
        sessions,
        tab_count: tabs.len(),
        ..Default::default()
    };
    if let Some(roster) = roster {
        let matched = |name: &str| -> (Option<PaneId>, String, Option<AgentSection>) {
            let Some(p) = pane_named(tabs, name) else { return (None, String::new(), None) };
            let row = vm.panes.iter().find(|r| r.pane_id == p.pane_id);
            let meta = row
                .map(|r| if r.meta.is_empty() { r.section.label().to_lowercase() } else { r.meta.clone() })
                .unwrap_or_default();
            (Some(p.pane_id), meta, row.map(|r| r.section))
        };
        for entry in &roster.managers {
            let (pane_id, meta, pane_state) = matched(&entry.name);
            let open = entry.state == RosterState::Open;
            vm.team.push(TeamRow {
                name: entry.name.clone(),
                role: entry.role.clone(),
                project: None,
                open,
                pane_id: if open { pane_id } else { None },
                meta: if open && meta.is_empty() { "open".to_string() } else { meta },
                pane_state,
                reopen: entry.reopen.clone(),
            });
        }
        for entry in &roster.dedicated {
            let (pane_id, meta, pane_state) = matched(&entry.name);
            let open = entry.state == RosterState::Open;
            vm.dedicated.push(TeamRow {
                name: entry.name.clone(),
                role: entry.role.clone(),
                project: entry.project.clone().filter(|p| !p.trim().is_empty()),
                open,
                pane_id: if open { pane_id } else { None },
                meta: if open && meta.is_empty() { "open".to_string() } else { meta },
                pane_state,
                reopen: entry.reopen.clone(),
            });
        }
    }
    vm
}

/// The pane a roster entry runs: the pane's agent session name, or a pane or
/// tab title, equal to the entry name, case-insensitively.
fn pane_named<'a>(tabs: &'a [TabFacts], name: &str) -> Option<&'a PaneFacts> {
    for tab in tabs {
        let tab_matches = tab_name(tab).eq_ignore_ascii_case(name);
        for p in &tab.panes {
            let names = [p.session_name.as_deref(), p.custom_title.as_deref(), p.osc_title.as_deref()]
                .into_iter()
                .flatten()
                .any(|t| t.trim().eq_ignore_ascii_case(name));
            if tab_matches || names {
                return Some(p);
            }
        }
    }
    None
}

// ---------------------------------------------------------------
// View state
// ---------------------------------------------------------------

/// What the view keeps between ticks: the selected stat tile, whether the
/// closed dedicated sessions are folded, the reopens in flight and the ones
/// that failed.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentsState {
    pub filter: AgentSection,
    /// `Closed N` starts folded: the closed sessions are the long tail.
    pub closed_folded: bool,
    /// Reopens in flight, by roster name, and when the button may try again.
    pub busy: Vec<(String, u64)>,
    /// A failed reopen's words, and when they stop showing.
    pub errors: Vec<(String, String, u64)>,
}

impl Default for AgentsState {
    fn default() -> Self {
        AgentsState { filter: AgentSection::Working, closed_folded: true, busy: Vec::new(), errors: Vec::new() }
    }
}

impl AgentsState {
    pub fn set_filter(&mut self, section: AgentSection) {
        self.filter = section;
    }

    pub fn toggle_closed(&mut self) {
        self.closed_folded = !self.closed_folded;
    }

    pub fn busy(&self, name: &str) -> bool {
        self.busy.iter().any(|(n, _)| n == name)
    }

    /// The button says what it is doing and ignores further clicks.
    pub fn start(&mut self, name: &str, at_ms: u64) {
        if !self.busy(name) {
            self.busy.push((name.to_string(), at_ms + OPEN_TIMEOUT_MS));
        }
    }

    /// Apply what a background reopen reported: the row stops saying what it
    /// is doing, and a failure keeps its words for `ERROR_MS`.
    pub fn finish(&mut self, outcome: &team::ReopenOutcome, at_ms: u64) {
        self.busy.retain(|(n, _)| n != &outcome.name);
        if outcome.result.is_err() {
            self.failed(&outcome.name, at_ms);
        }
    }

    /// A failure that never reached the CLI.
    pub fn failed(&mut self, name: &str, at_ms: u64) {
        self.busy.retain(|(n, _)| n != name);
        self.errors.retain(|(n, _, _)| n != name);
        self.errors.push((name.to_string(), FAILED_OPEN.to_string(), at_ms + ERROR_MS));
    }

    pub fn error(&self, name: &str) -> Option<&str> {
        self.errors.iter().find(|(n, _, _)| n == name).map(|(_, w, _)| w.as_str())
    }

    /// Drop the words whose time is up, and the busy marks of a roster entry
    /// the last read shows open (or whose 20 s ran out).
    pub fn prune(&mut self, at_ms: u64, open_names: &[String]) {
        self.busy.retain(|(n, until)| *until > at_ms && !open_names.iter().any(|o| o == n));
        self.errors.retain(|(_, _, until)| *until > at_ms);
    }
}

// ---------------------------------------------------------------
// Hit targets
// ---------------------------------------------------------------

/// Which roster list an `Open` button belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RosterRef {
    Manager(usize),
    Dedicated(usize),
}

/// What a point of the Agents list lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentsHit {
    /// A stat tile: select that filter.
    Filter(AgentSection),
    Pane(PaneId),
    /// A row of `Need you`, by its index in `AgentsVm::panes`: the ask or
    /// permission it names is what a click opens.
    Need(usize),
    /// A `By tab` row: switch to Panes on that tab.
    Tab(usize),
    /// The `Open` button of a closed roster entry.
    Open(RosterRef),
    /// The folded `Closed N` header.
    ToggleClosed,
}

/// What a point inside an Agents row lands on. The `Open` buttons come
/// first: they sit inside the row they belong to.
pub(super) fn hit(row: &Row, x: f64, y: f64, buttons: &[(AgentsHit, Rect)]) -> Option<AgentsHit> {
    if !row.frame.contains(x, y) {
        return None;
    }
    if let Some((hit, _)) = buttons.iter().find(|(_, rect)| rect.contains(x, y)) {
        return Some(*hit);
    }
    match row.kind {
        RowKind::AgentStat(section) => Some(AgentsHit::Filter(section)),
        RowKind::AgentPane { index, pane_id, need } => {
            Some(if need { AgentsHit::Need(index) } else { AgentsHit::Pane(pane_id) })
        }
        RowKind::AgentTab { tab_idx, .. } => Some(AgentsHit::Tab(tab_idx)),
        RowKind::AgentTeam { pane_id, .. } | RowKind::AgentDedicated { pane_id, .. } => pane_id.map(AgentsHit::Pane),
        RowKind::AgentHeader { kind: SectionKind::Closed, .. } => Some(AgentsHit::ToggleClosed),
        _ => None,
    }
}

// ---------------------------------------------------------------
// Layout
// ---------------------------------------------------------------

/// One section as laid out: its header and how many rows it holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgentsSection {
    pub kind: SectionKind,
    pub header: Rect,
    pub count: usize,
}

/// The Agents list's geometry at one width.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentsLayout {
    pub stats: Vec<Rect>,
    pub sections: Vec<AgentsSection>,
    /// The `Open` buttons of the closed roster rows: they sit inside their
    /// row, and the hit test tries them before the row itself.
    pub open_buttons: Vec<(AgentsHit, Rect)>,
    /// The same box while a reopen runs, saying `Opening…`: drawn disabled,
    /// never in the hit test.
    pub open_waiting: Vec<(AgentsHit, Rect)>,
    pub hint_y: Option<f64>,
    pub content_h: f64,
}

fn row(kind: RowKind, frame: Rect) -> Row {
    Row {
        kind,
        frame,
        glyphs: Vec::new(),
        actions: Vec::new(),
        detail: Rect::default(),
        answer: None,
        sched: Vec::new(),
        sched_confirm: None,
    }
}

/// Lay the Agents list out at `width`, filtered on `state.filter`.
pub fn layout(vm: &AgentsVm, state: &AgentsState, width: f64, m: &dyn TextMetrics) -> (AgentsLayout, Vec<Row>) {
    let x0 = PAD_H;
    let x1 = (width - (PAD_H - EDGE_TOLERANCE)).max(x0 + 40.0);
    let mut out = AgentsLayout::default();
    let mut rows: Vec<Row> = Vec::new();

    // The four stat cards: a 2 x 2 grid, or one row of four when the list is
    // wide enough for the label and its number side by side.
    let cols = if width >= STAT_WIDE { 4 } else { 2 };
    let card_w = ((x1 - x0) - (cols - 1) as f64 * STAT_GAP) / cols as f64;
    for (i, section) in AgentSection::ALL.iter().enumerate() {
        let frame = Rect::new(
            x0 + (i % cols) as f64 * (card_w + STAT_GAP),
            LIST_TOP + (i / cols) as f64 * (STAT_H + STAT_GAP),
            card_w,
            STAT_H,
        );
        out.stats.push(frame);
        rows.push(row(RowKind::AgentStat(*section), frame));
    }
    let grid_rows = AgentSection::ALL.len().div_ceil(cols);
    let mut y = LIST_TOP + grid_rows as f64 * STAT_H + grid_rows.saturating_sub(1) as f64 * STAT_GAP;

    // The selected stat's section: the rows the filter picked.
    y += SECTION_GAP;
    let filter = state.filter;
    let mut indices: Vec<usize> =
        vm.panes.iter().enumerate().filter(|(_, p)| p.section == filter).map(|(i, _)| i).collect();
    match filter {
        // Longest first: the pane that has been working the longest is the
        // one that deserves a look, not the one that just started.
        AgentSection::Working => indices.sort_by(|a, b| {
            let (a, b) = (&vm.panes[*a], &vm.panes[*b]);
            b.age_secs.unwrap_or(0).cmp(&a.age_secs.unwrap_or(0)).then_with(|| a.title.cmp(&b.title))
        }),
        // A wait that began first is the oldest and the most urgent.
        AgentSection::Need => indices.sort_by_key(|i| vm.panes[*i].since.unwrap_or(u64::MAX)),
        AgentSection::Idle | AgentSection::Error => {}
    }
    y = place_header(&mut out, &mut rows, SectionKind::Filter(filter), indices.len(), x0, x1, y);
    if indices.is_empty() {
        rows.push(row(RowKind::AgentEmpty, Rect::new(x0, y, x1 - x0, EMPTY_H)));
        y += EMPTY_H;
    } else {
        for index in indices {
            y += ROW_GAP;
            let frame = Rect::new(x0, y, x1 - x0, ROW_H);
            let pane = &vm.panes[index];
            rows.push(row(
                RowKind::AgentPane { index, pane_id: pane.pane_id, need: pane.section == AgentSection::Need },
                frame,
            ));
            y += ROW_H;
        }
    }

    // By tab: one row per tab, whatever the filter.
    y += SECTION_GAP;
    y = place_header(&mut out, &mut rows, SectionKind::ByTab, vm.tabs.len(), x0, x1, y);
    if vm.tabs.is_empty() {
        rows.push(row(RowKind::AgentEmpty, Rect::new(x0, y, x1 - x0, EMPTY_H)));
        y += EMPTY_H;
    } else {
        for (index, tab) in vm.tabs.iter().enumerate() {
            let frame = Rect::new(x0, y, x1 - x0, TAB_ROW_H);
            rows.push(row(RowKind::AgentTab { index, tab_idx: tab.tab_idx }, frame));
            y += TAB_ROW_H;
        }
    }

    if vm.team_loaded && vm.team_available {
        // Team: the managers, matched to the panes of this window.
        y += SECTION_GAP;
        y = place_header(&mut out, &mut rows, SectionKind::Team, vm.team.len(), x0, x1, y);
        if vm.team.is_empty() {
            rows.push(row(RowKind::AgentEmpty, Rect::new(x0, y, x1 - x0, EMPTY_H)));
            y += EMPTY_H;
        } else {
            for (index, entry) in vm.team.iter().enumerate() {
                y += ROW_GAP;
                let frame = Rect::new(x0, y, x1 - x0, ROW_H);
                place_open(&mut out, &frame, state, entry, RosterRef::Manager(index), m);
                rows.push(row(RowKind::AgentTeam { index, pane_id: entry.pane_id }, frame));
                y += ROW_H;
            }
        }
        // Dedicated sessions: open ones listed, closed ones folded away.
        if !vm.dedicated.is_empty() {
            y += SECTION_GAP;
            y = place_header(&mut out, &mut rows, SectionKind::Dedicated, vm.dedicated.len(), x0, x1, y);
            for (index, entry) in vm.dedicated.iter().enumerate().filter(|(_, e)| e.open) {
                y += ROW_GAP;
                let frame = Rect::new(x0, y, x1 - x0, ROW_H);
                rows.push(row(RowKind::AgentDedicated { index, pane_id: entry.pane_id }, frame));
                y += ROW_H;
            }
            let closed: Vec<usize> = vm.dedicated.iter().enumerate().filter(|(_, e)| !e.open).map(|(i, _)| i).collect();
            if !closed.is_empty() {
                y += ROW_GAP;
                y = place_header(&mut out, &mut rows, SectionKind::Closed, closed.len(), x0, x1, y);
                if !state.closed_folded {
                    for index in closed {
                        y += ROW_GAP;
                        let entry = &vm.dedicated[index];
                        let frame = Rect::new(x0, y, x1 - x0, ROW_H);
                        place_open(&mut out, &frame, state, entry, RosterRef::Dedicated(index), m);
                        rows.push(row(RowKind::AgentDedicated { index, pane_id: entry.pane_id }, frame));
                        y += ROW_H;
                    }
                }
            }
        }
    }

    out.content_h = y + LIST_BOTTOM;
    (out, rows)
}

fn place_header(
    out: &mut AgentsLayout,
    rows: &mut Vec<Row>,
    kind: SectionKind,
    count: usize,
    x0: f64,
    x1: f64,
    y: f64,
) -> f64 {
    let frame = Rect::new(x0, y, x1 - x0, SECTION_H);
    out.sections.push(AgentsSection { kind, header: frame, count });
    rows.push(row(RowKind::AgentHeader { kind, count }, frame));
    y + SECTION_H
}

/// The `Open` button of a closed row: no button while a reopen is in flight
/// or just failed, the words take its place there.
fn place_open(
    out: &mut AgentsLayout,
    frame: &Rect,
    state: &AgentsState,
    entry: &TeamRow,
    reference: RosterRef,
    m: &dyn TextMetrics,
) {
    if entry.open || state.error(&entry.name).is_some() {
        return;
    }
    let waiting = state.busy(&entry.name);
    let label = if waiting { OPENING } else { OPEN_LABEL };
    let w = m.width(label, Style::Chip) + 2.0 * OPEN_PAD;
    let rect = Rect::new(frame.right() - ROW_PAD_X - w, frame.y + (ROW_H - OPEN_H) / 2.0, w, OPEN_H);
    let hit = AgentsHit::Open(reference);
    if waiting {
        out.open_waiting.push((hit, rect));
    } else {
        out.open_buttons.push((hit, rect));
    }
}

// ---------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------

fn draw_text_sh(sh: &Shared, text: &str, r: &Rect, style: Style, c: [f32; 3], alpha: f64) -> f64 {
    draw_text(sh, text, r, style, c, alpha, Align::Left, false)
}

fn state_dot(cx: f64, cy: f64, section: AgentSection, alpha: f64) {
    if section == AgentSection::Idle {
        stroke_ring(cx, cy, 6.0, 1.5, tokens::get().text_tertiary, alpha);
    } else {
        fill_dot(cx, cy, DOT, section.color(), alpha);
    }
}

/// Draw one row of the Agents list.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    sh: &Shared,
    vm: &AgentsVm,
    state: &AgentsState,
    row: &Row,
    frame: &Rect,
    buttons: &[(AgentsHit, Rect)],
    waiting: &[(AgentsHit, Rect)],
    hovered: ListHit,
    pressed: ListHit,
    alpha: f64,
) {
    match row.kind {
        RowKind::AgentStat(section) => draw_stat(sh, vm, state, section, frame, alpha),
        RowKind::AgentHeader { kind, count } => draw_header(sh, kind, count, state, frame, alpha),
        RowKind::AgentEmpty => {
            draw_text_sh(
                sh,
                "None",
                &Rect::new(frame.x + 12.0, frame.y, (frame.w - 24.0).max(0.0), EMPTY_H),
                Style::Secondary,
                tokens::get().text_tertiary,
                alpha,
            );
        }
        RowKind::AgentPane { index, .. } => {
            let Some(pane) = vm.panes.get(index) else { return };
            let Some(this) = row_hit(row) else { return };
            let hovered = hovered == ListHit::Agent(this);
            let pressed = pressed == ListHit::Agent(this);
            draw_pane(sh, pane, frame, hovered, pressed, alpha);
        }
        RowKind::AgentTab { index, .. } => {
            let Some(tab) = vm.tabs.get(index) else { return };
            let this = AgentsHit::Tab(tab.tab_idx);
            draw_tab(sh, tab, frame, hovered == ListHit::Agent(this), pressed == ListHit::Agent(this), alpha);
        }
        RowKind::AgentTeam { index, .. } => {
            if let Some(entry) = vm.team.get(index) {
                draw_team(
                    sh,
                    entry,
                    state,
                    open_button(buttons, RosterRef::Manager(index)),
                    open_button(waiting, RosterRef::Manager(index)),
                    frame,
                    hovered,
                    pressed,
                    alpha,
                    RosterRef::Manager(index),
                );
            }
        }
        RowKind::AgentDedicated { index, .. } => {
            if let Some(entry) = vm.dedicated.get(index) {
                draw_team(
                    sh,
                    entry,
                    state,
                    open_button(buttons, RosterRef::Dedicated(index)),
                    open_button(waiting, RosterRef::Dedicated(index)),
                    frame,
                    hovered,
                    pressed,
                    alpha,
                    RosterRef::Dedicated(index),
                );
            }
        }
        _ => {}
    }
}

fn open_button(buttons: &[(AgentsHit, Rect)], reference: RosterRef) -> Option<Rect> {
    let this = AgentsHit::Open(reference);
    buttons.iter().find(|(hit, _)| *hit == this).map(|(_, rect)| *rect)
}

fn row_hit(row: &Row) -> Option<AgentsHit> {
    match row.kind {
        RowKind::AgentPane { index, pane_id, need } => {
            Some(if need { AgentsHit::Need(index) } else { AgentsHit::Pane(pane_id) })
        }
        RowKind::AgentStat(section) => Some(AgentsHit::Filter(section)),
        RowKind::AgentTab { tab_idx, .. } => Some(AgentsHit::Tab(tab_idx)),
        RowKind::AgentTeam { pane_id, .. } | RowKind::AgentDedicated { pane_id, .. } => pane_id.map(AgentsHit::Pane),
        RowKind::AgentHeader { kind: SectionKind::Closed, .. } => Some(AgentsHit::ToggleClosed),
        _ => None,
    }
}

fn draw_stat(sh: &Shared, vm: &AgentsVm, state: &AgentsState, section: AgentSection, frame: &Rect, alpha: f64) {
    fill_round(frame, STAT_RADIUS, tokens::get().tile, alpha);
    let selected = state.filter == section;
    stroke_round(frame, STAT_RADIUS, 1.0, if selected { tokens::get().sel_rail } else { tokens::get().border_subtle }, alpha);
    let value = vm.stat(section).to_string();
    draw_text_sh(
        sh,
        &value,
        &Rect::new(frame.x + STAT_PAD_X, frame.y + STAT_PAD_Y, (frame.w - 2.0 * STAT_PAD_X).max(0.0), STAT_VALUE_H),
        Style::StatValue,
        tokens::get().text_primary,
        alpha,
    );
    let label_y = frame.bottom() - STAT_PAD_Y - STAT_LABEL_H;
    let dot_x = frame.x + STAT_PAD_X + STAT_DOT / 2.0;
    fill_dot(dot_x, label_y + STAT_LABEL_H / 2.0, STAT_DOT, section.color(), alpha);
    draw_text_sh(
        sh,
        section.label(),
        &Rect::new(
            frame.x + STAT_PAD_X + STAT_DOT + 4.0,
            label_y,
            (frame.w - 2.0 * STAT_PAD_X - STAT_DOT - 4.0).max(0.0),
            STAT_LABEL_H,
        ),
        Style::Chip,
        tokens::get().text_secondary,
        alpha,
    );
}

fn draw_header(sh: &Shared, kind: SectionKind, count: usize, state: &AgentsState, frame: &Rect, alpha: f64) {
    let mut x = frame.x + SECTION_LABEL_X;
    let mut ink = tokens::get().text_secondary;
    let label = match kind {
        SectionKind::Closed => {
            let chevron = if state.closed_folded { Icon::ChevronRight } else { Icon::ChevronDown };
            stroke_icon(
                chevron,
                &Rect::new(frame.x + SECTION_CHEVRON_X, frame.y + (frame.h - CHEVRON) / 2.0, CHEVRON, CHEVRON),
                CHEVRON,
                tokens::get().text_tertiary,
                alpha,
            );
            x = frame.x + SECTION_FOLDED_X;
            format!("Closed {count}")
        }
        SectionKind::Filter(AgentSection::Need) => {
            ink = tokens::get().warn_text;
            kind.label().to_string()
        }
        SectionKind::Filter(AgentSection::Error) => {
            ink = tokens::get().crit_text;
            kind.label().to_string()
        }
        _ => kind.label().to_string(),
    };
    let w = draw_text_sh(sh, &label, &Rect::new(x, frame.y, (frame.w - (x - frame.x)).max(0.0), SECTION_H), Style::Chip, ink, alpha);
    if !matches!(kind, SectionKind::Closed) {
        draw_text_sh(
            sh,
            &count.to_string(),
            &Rect::new(x + w + SECTION_COUNT_GAP, frame.y, (frame.w - (x - frame.x) - w).max(0.0), SECTION_H),
            Style::Chip,
            tokens::get().text_tertiary,
            alpha,
        );
    }
    if let Some(note) = kind.note() {
        let m = AppKitMetrics(sh);
        let note_w = m.width(note, Style::Hint);
        draw_text(
            sh,
            note,
            &Rect::new(frame.right() - SECTION_PAD_X - note_w, frame.y, note_w, SECTION_H),
            Style::Hint,
            tokens::get().text_tertiary,
            alpha,
            Align::Right,
            false,
        );
    }
    if kind == SectionKind::ByTab {
        // Four small dots as the legend of the count columns, right to left.
        let mut cx = frame.right() - SECTION_PAD_X - 3.0;
        for section in TAB_COLUMNS.iter().rev() {
            state_dot(cx, frame.y + frame.h / 2.0, *section, alpha);
            cx -= 10.0;
        }
    }
}

fn draw_pane(sh: &Shared, pane: &AgentPane, frame: &Rect, hovered: bool, pressed: bool, alpha: f64) {
    if pressed {
        fill_round(frame, ROW_RADIUS, tokens::get().tile_pressed, alpha);
    } else if hovered {
        fill_round(frame, ROW_RADIUS, tokens::get().tile_hover, alpha);
    }
    let cy = frame.y + ROW_PAD_Y + LINE_H / 2.0;
    state_dot(frame.x + ROW_PAD_X + DOT_BOX / 2.0, cy, pane.section, alpha);
    let m = AppKitMetrics(sh);
    let right = frame.right() - ROW_PAD_X;
    let meta_w = if pane.meta.is_empty() { 0.0 } else { m.width(&pane.meta, Style::Number) };
    if !pane.meta.is_empty() {
        draw_text(
            sh,
            &pane.meta,
            &Rect::new(right - meta_w, frame.y + ROW_PAD_Y, meta_w, LINE_H),
            Style::Number,
            pane.section.ink(),
            alpha,
            Align::Right,
            false,
        );
    }
    let text_w = (right - meta_w - if meta_w > 0.0 { TEXT_GAP } else { 0.0 } - (frame.x + TEXT_X)).max(20.0);
    draw_text_sh(
        sh,
        &pane.title,
        &Rect::new(frame.x + TEXT_X, frame.y + ROW_PAD_Y, text_w, LINE_H),
        Style::Title,
        tokens::get().text_primary,
        alpha,
    );
    let line = if pane.detail.trim().is_empty() { pane.tab.clone() } else { format!("{} \u{b7} {}", pane.tab, pane.detail) };
    draw_text(
        sh,
        &line,
        &Rect::new(frame.x + TEXT_X, frame.y + ROW_PAD_Y + LINE_H, text_w, LINE_H),
        Style::Secondary,
        tokens::get().text_tertiary,
        alpha,
        Align::Left,
        false,
    );
}

fn draw_tab(sh: &Shared, tab: &AgentTab, frame: &Rect, hovered: bool, pressed: bool, alpha: f64) {
    if pressed {
        fill_round(frame, ROW_RADIUS, tokens::get().tile_pressed, alpha);
    } else if hovered {
        fill_round(frame, ROW_RADIUS, tokens::get().tile_hover, alpha);
    }
    let cy = frame.y + frame.h / 2.0;
    // The tab's colour, a filled dot: the marker the group headers use.
    fill_dot(frame.x + ROW_PAD_X + TAB_DOT_D / 2.0, cy, TAB_DOT_D, tab_tint(tab.color), alpha);
    let mut right = frame.right() - TAB_END_PAD;
    for section in TAB_COLUMNS.iter().rev() {
        let n = tab.count(*section);
        let r = Rect::new(right - TAB_COL_W, frame.y, TAB_COL_W, frame.h);
        if n > 0 {
            draw_text(sh, &n.to_string(), &r, Style::Number, section.ink(), alpha, Align::Right, false);
        }
        right -= TAB_COL_W;
    }
    draw_text(
        sh,
        &tab.title,
        &Rect::new(frame.x + TAB_TEXT_X, frame.y, (right - frame.x - TAB_TEXT_X).max(20.0), frame.h),
        Style::Title,
        tokens::get().text_primary,
        alpha,
        Align::Left,
        false,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_team(
    sh: &Shared,
    entry: &TeamRow,
    state: &AgentsState,
    open_button: Option<Rect>,
    open_waiting: Option<Rect>,
    frame: &Rect,
    hovered: ListHit,
    pressed: ListHit,
    alpha: f64,
    reference: RosterRef,
) {
    let body_hit = entry.pane_id.map(AgentsHit::Pane);
    let body_hovered = body_hit.is_some_and(|h| hovered == ListHit::Agent(h));
    let body_pressed = body_hit.is_some_and(|h| pressed == ListHit::Agent(h));
    if body_pressed {
        fill_round(frame, ROW_RADIUS, tokens::get().tile_pressed, alpha);
    } else if body_hovered {
        fill_round(frame, ROW_RADIUS, tokens::get().tile_hover, alpha);
    }
    let cy = frame.y + ROW_PAD_Y + LINE_H / 2.0;
    let a = if entry.open { alpha } else { alpha * 0.55 };
    match entry.pane_state.filter(|_| entry.open) {
        Some(section) => state_dot(frame.x + ROW_PAD_X + DOT_BOX / 2.0, cy, section, a),
        None => stroke_ring(frame.x + ROW_PAD_X + DOT_BOX / 2.0, cy, 6.0, 1.5, tokens::get().text_tertiary, a),
    }
    let right = frame.right() - ROW_PAD_X;
    let this = AgentsHit::Open(reference);
    let mut text_right = right;
    if let Some(button) = open_button {
        let hovered_here = hovered == ListHit::Agent(this);
        let pressed_here = pressed == ListHit::Agent(this);
        // The Scheduled body's small button: a ground at rest, darker under
        // the mouse and the press, an 11 pt Medium label on it.
        let ground = if hovered_here || pressed_here { tokens::get().tile_pressed } else { tokens::get().tile_hover };
        fill_round(&button, OPEN_RADIUS, ground, alpha);
        draw_text(sh, OPEN_LABEL, &button, Style::Chip, tokens::get().text_primary, alpha, Align::Center, false);
        text_right = button.x - TEXT_GAP;
    } else if let Some(button) = open_waiting {
        // The same box, disabled: the reopen is in flight, a click would only
        // start it twice.
        fill_round(&button, OPEN_RADIUS, tokens::get().tile_hover, alpha);
        draw_text(sh, OPENING, &button, Style::Chip, tokens::get().text_tertiary, alpha, Align::Center, false);
        text_right = button.x - TEXT_GAP;
    } else if let Some(words) = state.error(&entry.name) {
        let m = AppKitMetrics(sh);
        let w = m.width(words, Style::Chip);
        draw_text(sh, words, &Rect::new(right - w, frame.y, w, ROW_H), Style::Chip, tokens::get().crit_text, alpha, Align::Right, false);
        text_right = right - w - TEXT_GAP;
    } else if entry.open && !entry.meta.is_empty() {
        let m = AppKitMetrics(sh);
        let w = m.width(&entry.meta, Style::Number);
        let ink = entry.pane_state.map_or(tokens::get().text_tertiary, |s| s.ink());
        draw_text(
            sh,
            &entry.meta,
            &Rect::new(right - w, frame.y + ROW_PAD_Y, w, LINE_H),
            Style::Number,
            ink,
            alpha,
            Align::Right,
            false,
        );
        text_right = right - w - TEXT_GAP;
    }
    let text_w = (text_right - (frame.x + TEXT_X)).max(20.0);
    draw_text_sh(sh, &entry.name, &Rect::new(frame.x + TEXT_X, frame.y + ROW_PAD_Y, text_w, LINE_H), Style::Title, tokens::get().text_primary, alpha);
    let mut line = entry.role.clone();
    if let Some(project) = &entry.project {
        line = if line.is_empty() { project.clone() } else { format!("{line} \u{b7} {project}") };
    }
    if !entry.open {
        let words = if line.is_empty() { "not open".to_string() } else { format!("{line} \u{b7} not open") };
        draw_text_sh(
            sh,
            &words,
            &Rect::new(frame.x + TEXT_X, frame.y + ROW_PAD_Y + LINE_H, text_w, LINE_H),
            Style::Secondary,
            tokens::get().text_tertiary,
            a,
        );
    } else {
        draw_text(
            sh,
            &line,
            &Rect::new(frame.x + TEXT_X, frame.y + ROW_PAD_Y + LINE_H, text_w, LINE_H),
            Style::Secondary,
            tokens::get().text_tertiary,
            alpha,
            Align::Left,
            false,
        );
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::sidebar::PaneFlags;

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

    const NOW: u64 = 1_000;

    fn pane(id: u32, name: &str, flags: PaneFlags) -> PaneFacts {
        PaneFacts { pane_id: id, flags, session_name: Some(name.to_string()), ..Default::default() }
    }

    fn tab(idx: usize, title: &str, panes: Vec<PaneFacts>) -> TabFacts {
        TabFacts { tab_idx: idx, tab_id: idx as u32, title: title.to_string(), panes, ..Default::default() }
    }

    fn working() -> PaneFlags {
        PaneFlags { working: true, ..Default::default() }
    }

    fn failed() -> PaneFlags {
        PaneFlags { error: true, ..Default::default() }
    }

    fn idle() -> PaneFlags {
        PaneFlags { idle_agent: true, ..Default::default() }
    }

    fn ask_item(id: &str, pane_id: u32, since: u64) -> NeedItem {
        NeedItem {
            kind: NeedKind::Ask,
            key: format!("ask:{id}"),
            pane_id: Some(u64::from(pane_id)),
            since,
            title: "the asker".into(),
            detail: "proceed?".into(),
        }
    }

    /// Two tabs: two working panes, one idle, one failed, one bare shell,
    /// and an ask waiting on the idle pane.
    fn facts() -> Vec<TabFacts> {
        vec![
            tab(0, "Alpha", vec![pane(1, "worker", working()), pane(2, "senior", working())]),
            tab(1, "Beta", vec![pane(3, "sleeper", idle()), pane(4, "broken", failed()), pane(5, "shell", PaneFlags::default())]),
        ]
    }

    fn clock() -> AgentsClock {
        let mut clock = AgentsClock::default();
        clock.observe(&facts(), 0);
        clock
    }

    fn roster() -> team::Team {
        team::Team::from_json(include_str!("../../tests/fixtures/team/list.json")).expect("the roster fixture")
    }

    fn built(roster: Option<&team::Team>) -> AgentsVm {
        build(&facts(), &[ask_item("a", 3, 900)], &clock(), NOW, roster)
    }

    fn layouts(vm: &AgentsVm, state: &AgentsState, width: f64) -> (AgentsLayout, Vec<Row>) {
        layout(vm, state, width, &FakeMetrics)
    }

    fn row_of(rows: &[Row], kind: RowKind) -> &Row {
        rows.iter().find(|r| r.kind == kind).expect("a laid-out row")
    }

    fn pane_in(vm: &AgentsVm, id: PaneId) -> &AgentPane {
        vm.panes.iter().find(|p| p.pane_id == id).expect("the pane")
    }

    fn pane_ids(rows: &[Row], vm: &AgentsVm) -> Vec<PaneId> {
        rows.iter()
            .filter_map(|r| match r.kind {
                RowKind::AgentPane { index, .. } => Some(vm.panes[index].pane_id),
                _ => None,
            })
            .collect()
    }

    fn section_kinds(l: &AgentsLayout) -> Vec<SectionKind> {
        l.sections.iter().map(|s| s.kind).collect()
    }

    #[test]
    fn the_counts_come_from_the_panes_and_the_asks() {
        let vm = built(Some(&roster()));
        assert_eq!(vm.stat(AgentSection::Working), 2);
        assert_eq!(vm.stat(AgentSection::Need), 1);
        assert_eq!(vm.stat(AgentSection::Idle), 1);
        assert_eq!(vm.stat(AgentSection::Error), 1);
        assert_eq!((vm.sessions, vm.tab_count), (5, 2));
        assert_eq!(summary_words(&vm), "5 sessions in 2 tabs");
        // The waiting row is named by the question's author, and carries the
        // wait's age and the key a click opens.
        let waiting = pane_in(&vm, 3);
        assert_eq!(waiting.section, AgentSection::Need);
        assert_eq!(waiting.title, "the asker");
        assert_eq!(waiting.meta, "1m");
        assert_eq!(waiting.key.as_deref(), Some("ask:a"));
        // One session and one tab read in the singular.
        let one = vec![tab(0, "Solo", vec![pane(1, "one", working())])];
        let vm = build(&one, &[], &AgentsClock::default(), NOW, None);
        assert_eq!(summary_words(&vm), "1 session in 1 tab");
    }

    #[test]
    fn a_stat_tile_filters_the_section_under_it() {
        let vm = built(None);
        let mut state = AgentsState::default();
        assert_eq!(state.filter, AgentSection::Working);
        let (l, rows) = layouts(&vm, &state, 300.0);
        assert_eq!(l.sections[0].kind, SectionKind::Filter(AgentSection::Working));
        assert_eq!(l.sections[0].count, 2);
        assert_eq!(pane_ids(&rows, &vm), [2, 1], "equal ages fall back to the title");

        state.set_filter(AgentSection::Need);
        let (l, rows) = layouts(&vm, &state, 300.0);
        assert_eq!(l.sections[0].kind, SectionKind::Filter(AgentSection::Need));
        assert_eq!(pane_ids(&rows, &vm), [3]);

        // A filter with nothing in it keeps its header and says `None`.
        let one = vec![tab(0, "Solo", vec![pane(9, "solo", working())])];
        let only_working = build(&one, &[], &AgentsClock::default(), NOW, None);
        let mut state = AgentsState::default();
        state.set_filter(AgentSection::Idle);
        let (l, rows) = layouts(&only_working, &state, 300.0);
        assert_eq!(l.sections[0].count, 0);
        assert!(rows.iter().any(|r| r.kind == RowKind::AgentEmpty));
        // All four tiles stay, whichever one is selected.
        assert_eq!(l.stats.len(), 4);
    }

    #[test]
    fn the_sections_read_in_the_prototype_order() {
        let vm = built(Some(&roster()));
        let state = AgentsState::default();
        let (l, _) = layouts(&vm, &state, 300.0);
        assert_eq!(
            section_kinds(&l),
            [
                SectionKind::Filter(AgentSection::Working),
                SectionKind::ByTab,
                SectionKind::Team,
                SectionKind::Dedicated,
                SectionKind::Closed
            ]
        );
        // Without a roster there is no Team and no Dedicated section.
        let (plain, _) = layouts(&built(None), &state, 300.0);
        assert_eq!(section_kinds(&plain), [SectionKind::Filter(AgentSection::Working), SectionKind::ByTab]);
    }

    #[test]
    fn working_rows_are_longest_first_and_waits_oldest_first() {
        let mut clock = AgentsClock::default();
        let first = vec![tab(0, "Alpha", vec![pane(1, "first", working())])];
        clock.observe(&first, 500);
        let both = vec![tab(0, "Alpha", vec![pane(1, "first", working()), pane(2, "second", working())])];
        clock.observe(&both, 900);
        let vm = build(&both, &[], &clock, NOW, None);
        assert_eq!(pane_in(&vm, 1).age_secs, Some(500));
        assert_eq!(pane_in(&vm, 2).age_secs, Some(100));
        let state = AgentsState::default();
        let (_, rows) = layouts(&vm, &state, 300.0);
        assert_eq!(pane_ids(&rows, &vm), [1, 2]);

        // Two asks on two panes: the one that has waited since 100 comes
        // before the one from 900.
        let waiting = vec![tab(0, "Alpha", vec![pane(3, "old", idle()), pane(4, "new", idle())])];
        let items = [ask_item("late", 3, 900), ask_item("early", 4, 100)];
        let vm = build(&waiting, &items, &AgentsClock::default(), NOW, None);
        let mut state = AgentsState::default();
        state.set_filter(AgentSection::Need);
        let (_, rows) = layouts(&vm, &state, 300.0);
        assert_eq!(pane_ids(&rows, &vm), [4, 3]);
    }

    #[test]
    fn by_tab_rows_count_the_four_states() {
        let vm = built(None);
        let alpha = vm.tabs.iter().find(|t| t.title == "Alpha").expect("the Alpha tab");
        assert_eq!(
            [AgentSection::Working, AgentSection::Need, AgentSection::Idle, AgentSection::Error]
                .map(|s| alpha.count(s)),
            [2, 0, 0, 0]
        );
        let beta = vm.tabs.iter().find(|t| t.title == "Beta").expect("the Beta tab");
        assert_eq!(
            [AgentSection::Working, AgentSection::Need, AgentSection::Idle, AgentSection::Error]
                .map(|s| beta.count(s)),
            [0, 1, 1, 1]
        );
    }

    #[test]
    fn a_roster_entry_matches_a_pane_by_session_or_tab_name() {
        // The open manager is the pane's session name, in another case.
        let facts = vec![tab(0, "Work", vec![pane(7, "ALPHA-MANAGER", idle())])];
        let vm = build(&facts, &[], &AgentsClock::default(), NOW, Some(&roster()));
        let alpha = vm.team.iter().find(|r| r.name == "alpha-manager").expect("the open manager");
        assert_eq!(alpha.pane_id, Some(7));
        assert_eq!(alpha.pane_state, Some(AgentSection::Idle));
        assert_eq!(alpha.meta, "idle");

        // The tab's own title matches too, but a closed entry never gets a
        // live pane.
        let facts = vec![tab(0, "BETA-COACH", vec![pane(8, "anything", idle())])];
        let vm = build(&facts, &[], &AgentsClock::default(), NOW, Some(&roster()));
        let beta = vm.team.iter().find(|r| r.name == "beta-coach").expect("the closed manager");
        assert_eq!(beta.pane_id, None);
        // With no pane carrying either name, an open entry matches nothing.
        let vm = built(Some(&roster()));
        assert!(vm.team.iter().filter(|r| r.open).all(|r| r.pane_id.is_none()));
    }

    #[test]
    fn a_closed_dedicated_starts_folded_and_opens_with_its_button() {
        let vm = built(Some(&roster()));
        let mut state = AgentsState::default();
        let (l, rows) = layouts(&vm, &state, 300.0);
        let closed = l.sections.iter().find(|s| s.kind == SectionKind::Closed).expect("the folded header");
        assert_eq!(closed.count, 1);
        let delta = vm.dedicated.iter().position(|r| r.name == "delta-project").expect("the closed session");
        assert!(rows.iter().all(|r| !matches!(r.kind, RowKind::AgentDedicated { index, .. } if index == delta)));
        let header = row_of(&rows, RowKind::AgentHeader { kind: SectionKind::Closed, count: 1 });
        assert_eq!(hit(header, header.frame.x + 4.0, header.frame.y + 4.0, &l.open_buttons), Some(AgentsHit::ToggleClosed));

        state.toggle_closed();
        let (l, rows) = layouts(&vm, &state, 300.0);
        let row = row_of(&rows, RowKind::AgentDedicated { index: delta, pane_id: None });
        let (button, rect) = *l
            .open_buttons
            .iter()
            .find(|(h, _)| *h == AgentsHit::Open(RosterRef::Dedicated(delta)))
            .expect("the dedicated session's Open button");
        assert_eq!(hit(row, rect.x + 2.0, rect.y + 2.0, &l.open_buttons), Some(button));
        // The row around the button is not a pane.
        assert_eq!(hit(row, row.frame.x + 4.0, row.frame.y + 4.0, &l.open_buttons), None);
    }

    #[test]
    fn a_closed_manager_offers_open_and_an_open_one_focuses_its_pane() {
        let facts = vec![tab(0, "Alpha", vec![pane(7, "alpha-manager", idle())])];
        let vm = build(&facts, &[], &AgentsClock::default(), NOW, Some(&roster()));
        let state = AgentsState::default();
        let (l, rows) = layouts(&vm, &state, 300.0);
        let beta = vm.team.iter().position(|r| r.name == "beta-coach").expect("the closed manager");
        let closed = row_of(&rows, RowKind::AgentTeam { index: beta, pane_id: None });
        let (button, rect) = *l
            .open_buttons
            .iter()
            .find(|(h, _)| *h == AgentsHit::Open(RosterRef::Manager(beta)))
            .expect("the closed manager's Open button");
        assert_eq!(hit(closed, rect.x + 2.0, rect.y + 2.0, &l.open_buttons), Some(button));
        assert_eq!(hit(closed, closed.frame.x + 4.0, closed.frame.y + 4.0, &l.open_buttons), None);

        let alpha = vm.team.iter().position(|r| r.name == "alpha-manager").expect("the open manager");
        let open = row_of(&rows, RowKind::AgentTeam { index: alpha, pane_id: Some(7) });
        assert!(l.open_buttons.iter().all(|(h, _)| *h != AgentsHit::Open(RosterRef::Manager(alpha))));
        assert_eq!(hit(open, open.frame.x + 4.0, open.frame.y + 4.0, &l.open_buttons), Some(AgentsHit::Pane(7)));
    }

    #[test]
    fn an_opening_row_shows_the_open_box_disabled() {
        let vm = built(Some(&roster()));
        let mut state = AgentsState::default();
        let beta = vm.team.iter().position(|r| r.name == "beta-coach").expect("the closed manager");
        state.start("beta-coach", 1_000);
        let (l, rows) = layouts(&vm, &state, 300.0);
        // No clickable button while the reopen runs: a second click would
        // start it twice.
        assert!(l.open_buttons.iter().all(|(h, _)| *h != AgentsHit::Open(RosterRef::Manager(beta))));
        let (_, rect) = *l
            .open_waiting
            .iter()
            .find(|(h, _)| *h == AgentsHit::Open(RosterRef::Manager(beta)))
            .expect("the disabled Opening box");
        assert_eq!(rect.h, OPEN_H);
        // It wears the same box, `Opening…` being wider than `Open`.
        let plain = FakeMetrics.width(OPEN_LABEL, Style::Chip) + 2.0 * OPEN_PAD;
        let words = FakeMetrics.width(OPENING, Style::Chip) + 2.0 * OPEN_PAD;
        assert_eq!(rect.w, words);
        assert!(words > plain);
        // Inside the row, but never a target.
        let row = row_of(&rows, RowKind::AgentTeam { index: beta, pane_id: None });
        assert!(row.frame.contains(rect.x + 2.0, rect.y + 2.0));
        assert_eq!(hit(row, rect.x + 2.0, rect.y + 2.0, &l.open_buttons), None);
    }

    #[test]
    fn stat_tab_and_need_rows_each_have_their_hit() {
        let vm = built(Some(&roster()));
        let mut state = AgentsState::default();
        let (l, rows) = layouts(&vm, &state, 300.0);
        for (i, section) in AgentSection::ALL.iter().enumerate() {
            let r = row_of(&rows, RowKind::AgentStat(*section));
            assert_eq!(r.frame, l.stats[i]);
            assert_eq!(hit(r, r.frame.x + 2.0, r.frame.y + 2.0, &l.open_buttons), Some(AgentsHit::Filter(*section)));
        }
        let tab = vm.tabs.first().expect("a tab");
        let tab_row = row_of(&rows, RowKind::AgentTab { index: 0, tab_idx: tab.tab_idx });
        assert_eq!(
            hit(tab_row, tab_row.frame.x + 2.0, tab_row.frame.y + 2.0, &l.open_buttons),
            Some(AgentsHit::Tab(tab.tab_idx))
        );

        state.set_filter(AgentSection::Need);
        let (l, rows) = layouts(&vm, &state, 300.0);
        let index = vm.panes.iter().position(|p| p.pane_id == 3).expect("the waiting pane");
        let r = row_of(&rows, RowKind::AgentPane { index, pane_id: 3, need: true });
        assert_eq!(hit(r, r.frame.x + 4.0, r.frame.y + 4.0, &l.open_buttons), Some(AgentsHit::Need(index)));
    }

    #[test]
    fn the_stat_grid_is_two_by_two_below_360_and_one_row_above() {
        let vm = built(None);
        let state = AgentsState::default();
        let (narrow, _) = layouts(&vm, &state, 300.0);
        assert_eq!(narrow.stats[0].y, LIST_TOP);
        assert_eq!(narrow.stats[1].y, LIST_TOP);
        assert_eq!(narrow.stats[2].y, LIST_TOP + STAT_H + STAT_GAP);
        assert_eq!(narrow.stats[0].w, narrow.stats[1].w);

        let (wide, rows) = layouts(&vm, &state, 420.0);
        assert!(wide.stats.iter().all(|r| r.y == LIST_TOP));
        assert_eq!(wide.stats[1].x, wide.stats[0].right() + STAT_GAP);
        assert!(wide.content_h < narrow.content_h, "one row of cards is shorter than two");
        assert_eq!(wide.content_h, rows.last().expect("a last row").frame.bottom() + LIST_BOTTOM);
    }

    #[test]
    fn a_section_header_follows_the_previous_content_by_one_gap() {
        let vm = built(Some(&roster()));
        let state = AgentsState::default();
        let (l, _) = layouts(&vm, &state, 420.0);
        assert_eq!(l.sections[0].header.y, LIST_TOP + STAT_H + SECTION_GAP);
        for pair in l.sections.windows(2) {
            assert!(pair[1].header.y > pair[0].header.bottom());
        }
    }

    #[test]
    fn the_reopen_button_says_what_it_is_doing_and_gives_up_after_twenty_seconds() {
        let mut state = AgentsState::default();
        state.start("delta-project", 1_000);
        state.start("delta-project", 2_000);
        assert!(state.busy("delta-project"));
        assert_eq!(state.busy.len(), 1);
        // The refresh showing it open clears the wait.
        state.prune(1_500, &["delta-project".to_string()]);
        assert!(!state.busy("delta-project"));
        // The deadline lets a stuck reopen be tried again.
        state.start("delta-project", 1_000);
        state.prune(1_000 + OPEN_TIMEOUT_MS, &[]);
        assert!(!state.busy("delta-project"));

        let failed = team::ReopenOutcome { name: "delta-project".into(), result: Err("no".into()) };
        state.start("delta-project", 3_000);
        state.finish(&failed, 3_000);
        assert!(!state.busy("delta-project"));
        assert_eq!(state.error("delta-project"), Some("Could not open"));
        state.prune(3_000 + ERROR_MS, &[]);
        assert_eq!(state.error("delta-project"), None);
    }

    #[test]
    fn the_working_and_idle_ages_move_with_the_clock() {
        let mut clock = AgentsClock::default();
        clock.observe(&facts(), 400);
        let vm = build(&facts(), &[], &clock, 1_000, None);
        let worker = pane_in(&vm, 1);
        assert_eq!(worker.section, AgentSection::Working);
        assert_eq!(worker.meta, "10m");
        assert_eq!(worker.age_secs, Some(600));
        assert_eq!(pane_in(&vm, 5).section, AgentSection::Idle);
        // A pane that leaves the window drops its mark.
        clock.observe(&[tab(0, "Alpha", vec![pane(1, "worker", working())])], 900);
        assert_eq!(clock.age(2, Activity::Working, 1_000), None);
        // A pane that goes idle starts a fresh idle age.
        clock.observe(&[tab(0, "Alpha", vec![pane(1, "worker", idle())])], 950);
        assert_eq!(clock.age(1, Activity::Idle, 1_000), Some(50));
        assert_eq!(clock.age(1, Activity::Working, 1_000), None);
    }
}
