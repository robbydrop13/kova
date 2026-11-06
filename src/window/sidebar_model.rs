//! The sidebar's view model: a plain, comparable description of what the
//! AppKit sidebar shows for one window, built once per tick from the tabs.
//! The view re-lays itself out only when the model changed. The reads from
//! `Pane` happen in `sidebar_ui.rs` (`PaneFacts`, `TabFacts`); everything from
//! the facts to the model is pure and tested here.

use super::needs::{NeedItem, NeedKind};
use super::sidebar_agents::{AgentsState, AgentsVm};
use super::sidebar_sched::SchedState;
use super::sidebar::{
    display_order, format_age, summary_runs, CollapsedSummary, NextPill, PaneFlags, SidebarSort,
    SummaryRun, TileState,
};
#[cfg(test)]
use super::sidebar::UnreadKind;
use crate::pane::{PaneId, TabId};
use crate::prompt_preview::PromptPreview;

/// Which list the sidebar is showing. The chrome above the list is the same
/// either way; only the scroll view's contents change. Named for the list,
/// not the view: `SidebarView` is already the AppKit class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidebarList {
    /// The panes of this window, grouped by tab.
    #[default]
    Panes,
    /// The agents running in this window.
    Agents,
    /// The scheduled local routines (`crate::routines`).
    Scheduled,
}

impl SidebarList {
    /// The title the chrome shows above the list.
    pub fn title(self) -> &'static str {
        match self {
            SidebarList::Panes => "Panes",
            SidebarList::Agents => "Agents",
            SidebarList::Scheduled => "Scheduled",
        }
    }
}

/// One routine as the Scheduled view shows it: its name, its section, the
/// words its two lines and its body read, and where its log lives. The prompt
/// is named by the view, never carried here: a full prompt in the model would
/// be cloned on every tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoutineVm {
    pub name: String,
    /// Clicked open: the row also shows its body.
    pub expanded: bool,
    /// Where it runs; empty while the row is closed, so an unopened list
    /// stays as cheap to compare as it was.
    pub cwd: String,
    /// Where it belongs in the Scheduled list, from `routines::group`.
    pub group: crate::routines::Group,
    /// The schedule as routinectl phrases it ("once, 6 Nov 09:47").
    pub schedule_words: String,
    /// When it next runs: `19:05`, `Wed 09:03`, `9 Nov`.
    pub next_words: String,
    /// How long until then: `in 20 min`.
    pub in_words: String,
    /// How the last run ended, running included.
    pub last_status: crate::routines::LastStatus,
    /// How long ago it ended: `10 min ago`.
    pub ago_words: String,
    /// The next run itself, not only its words: a section lists its rows in
    /// next-run order, and comparing words would compare "9 Nov" with "09:05".
    pub next_at: Option<crate::routines::When>,
    /// The log routinectl reports; empty when it reports none, the view then
    /// falls back to `logs/<name>.log`.
    pub log: String,
}

impl RoutineVm {
    /// The row at the current clock. The clock is only read once per row, so
    /// a list built on one tick phrases every routine against the same now.
    pub fn of(r: &crate::routines::Routine, expanded: bool) -> Self {
        Self::at(r, expanded, crate::routines::When::now())
    }

    pub fn at(r: &crate::routines::Routine, expanded: bool, now: crate::routines::When) -> Self {
        use crate::routines::{ago_words, group, in_words, next_words, LastStatus};
        let last_status = match &r.last {
            _ if r.running => LastStatus::Running,
            Some(l) if l.code != 0 => LastStatus::Failed(l.code),
            Some(_) => LastStatus::Ok,
            None => LastStatus::Never,
        };
        let last_at = r.last.as_ref().and_then(|l| l.at());
        RoutineVm {
            expanded,
            cwd: if expanded { r.cwd.clone() } else { String::new() },
            name: r.name.clone(),
            group: group(r, now),
            schedule_words: if r.schedule_words.is_empty() { r.schedule.clone() } else { r.schedule_words.clone() },
            next_words: r.next_run.map(|n| next_words(n, now)).unwrap_or_default(),
            in_words: r.next_run.map(|n| in_words(n, now)).unwrap_or_default(),
            last_status,
            ago_words: last_at.map(|w| ago_words(w, now)).unwrap_or_default(),
            next_at: r.next_run,
            log: r.log.clone(),
        }
    }
}

/// The Cockpit entry at the top of the Panes list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CockpitVm {
    /// `needs::waits_on_you`: what a human has to answer.
    pub count: u32,
    /// At least one action is urgent (priority 1): the red dot.
    pub urgent: bool,
    /// Its page is open over the panes: the entry is selected.
    pub open: bool,
    /// Kova records the screen (`crate::record`): a red dot takes the
    /// inbox icon's place, seen from any page.
    pub recording: bool,
}

/// One need, as the tiles' `Answer` link and the Agents view look it up.
#[derive(Clone, Debug, PartialEq)]
pub struct NeedRowVm {
    pub kind: NeedKind,
    /// `pane:12`, `routine:<name>`, `ask:<id>`: what a click opens.
    pub key: String,
    pub pane_id: Option<PaneId>,
    /// The pane, session or job the row is about.
    pub title: String,
    /// The line under the title: the provider's words, the command a
    /// permission wants, the rest of a question. Empty when there is none.
    pub detail: String,
    /// `4m`: how long it waits. The block never grows old between two ticks.
    pub age: String,
}

/// The block's rows at `now`, in need order.
pub fn need_rows(items: &[NeedItem], now: u64) -> Vec<NeedRowVm> {
    items
        .iter()
        .map(|item| NeedRowVm {
            kind: item.kind,
            key: item.key.clone(),
            pane_id: item.pane_id.and_then(|id| PaneId::try_from(id).ok()),
            title: item.title.clone(),
            detail: item.detail.clone(),
            age: format_age(now.saturating_sub(item.since)),
        })
        .collect()
}

/// What the Next pill shows, the same rule as `Cmd+J`: a need item with a
/// pane to land on comes first, then unread output. The count is what waits
/// on you among the stops it walks (asks and permissions: an attention entry
/// has no pane, Next never lands on it), never zero while
/// there is a stop, and the unread count when nothing needs an answer.
pub fn next_pill(needs: &[NeedItem], unread: usize) -> NextPill {
    if needs.iter().any(|item| item.pane_id.is_some()) {
        let waiting =
            needs.iter().filter(|item| matches!(item.kind, NeedKind::Ask | NeedKind::Permission)).count();
        NextPill::Next(waiting.max(1))
    } else {
        NextPill::of(unread)
    }
}

/// Everything the sidebar shows for one window.
#[derive(Clone, Debug, PartialEq)]
pub struct SidebarModel {
    /// Which list is showing; the switcher in the top right corner picks it.
    pub view: SidebarList,
    /// The routines, read only while the Scheduled view is up.
    pub routines: Vec<RoutineVm>,
    /// False until the first background read lands: an empty list then means
    /// "not read yet", not "nothing scheduled".
    pub routines_loaded: bool,
    /// A scheduled routine's last run failed: the red dot on its icon.
    /// `build` leaves it false; `sync_sidebar` fills it from `sync_needs`.
    pub routine_failed: bool,
    /// Panes of this window whose agent is working (`list-panes` `working`):
    /// the count on the Agents icon.
    pub agents_working: u32,
    /// `2 waiting · 3 working · 4 idle`, one coloured run at a time.
    pub summary: Vec<(String, SummaryRun)>,
    pub sort: SidebarSort,
    pub pill: NextPill,
    /// The groups in display order.
    pub groups: Vec<GroupVm>,
    /// The need items, in need order, for the tiles' `Answer` link. `build`
    /// leaves it empty; `apply_needs` fills it, with the tiles' `waiting` line.
    pub needs: Vec<NeedRowVm>,
    /// The Cockpit entry, when the list shows it. `build` leaves it out;
    /// `sync_sidebar` sets it on the Panes list.
    pub cockpit: Option<CockpitVm>,
    /// Fewer than three panes in the window: show the `⌘T new tab · ⌘D split` hint.
    pub show_hint: bool,
    /// The Scheduled view's folding, in-flight actions and inline
    /// confirmation; empty for every other list.
    pub sched: SchedState,
    /// The Scheduled list's meta row, each run with whether it reads as a
    /// failure. Empty for every other list.
    pub sched_meta: Vec<(String, bool)>,
    /// The Agents list's panes, tabs and roster. Empty for every other list.
    pub agents: AgentsVm,
    /// The Agents view's selected stat tile and folding, in flight reopens
    /// included; default for every other list.
    pub agent_state: AgentsState,
    /// The word the meta row keeps at its right. Only the Scheduled list has
    /// one (`now 18:45`); the panes draw the sort button there instead.
    pub meta_right: Option<String>,
}

/// One tab as a group header and its tiles.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupVm {
    pub tab_idx: usize,
    pub tab_id: TabId,
    /// Tab title, already the edit buffer while renaming.
    pub title: String,
    pub color: Option<usize>,
    pub active: bool,
    pub collapsed: bool,
    pub renaming: bool,
    pub summary: CollapsedSummary,
    /// Empty when collapsed.
    pub tiles: Vec<TileVm>,
}

/// One pane tile.
#[derive(Clone, Debug, PartialEq)]
pub struct TileVm {
    pub pane_id: PaneId,
    /// The column the pane sits in: a drag only reorders within one column.
    pub column: usize,
    pub state: TileState,
    /// `PaneFlags::is_unread`: the hover action reads Mark read / unread.
    pub unread: bool,
    pub minimized: bool,
    /// A plain shell: `▶ Start Claude` applies.
    pub bare_shell: bool,
    /// A shell with an agent's resume line waiting at its prompt: the chip
    /// names the agent and `▶ Resume` replaces `▶ Start Claude`.
    pub resumable: bool,
    /// The session name, else the pane's own title, else the agent, else
    /// `Shell` (`tile_title`); already the edit buffer while renaming.
    pub title: String,
    /// What the row used to say under the name, now its tooltip:
    /// `project · agent` (the agent left out when it is the title), or an
    /// errored pane's `kind · text`.
    pub tooltip: String,
    /// The live or restored agent, for the chip.
    pub agent: Option<String>,
    pub bookmarked: bool,
    /// The focused pane of the active tab.
    pub focused: bool,
    pub renaming: bool,
    /// Awaiting tile: the permission prompt's question and its detail (the
    /// command, the file, or the header when there is nothing else).
    pub question: Option<String>,
    pub detail: Option<String>,
    /// Unread tile: the first line of Claude's last answer.
    pub summary: Option<String>,
    /// A pane with an open ask or a permission prompt: the age word (`4m`)
    /// its subtitle shows as `waiting 4m · Answer`. `apply_needs` fills it,
    /// from the one need item that pane has.
    pub waiting: Option<String>,
}

/// The reads of one `Pane` the model needs.
#[derive(Clone, Debug, Default)]
pub struct PaneFacts {
    pub pane_id: PaneId,
    pub column: usize,
    pub flags: PaneFlags,
    pub minimized: bool,
    pub bare_shell: bool,
    /// `Pane::agent_session_name()`: what `/rename` called the conversation.
    pub session_name: Option<String>,
    /// `Pane::custom_title`, which the shell's OSC 1 also writes.
    pub custom_title: Option<String>,
    /// `Pane::osc_title()`.
    pub osc_title: Option<String>,
    /// The rename edit buffer while the pane is being renamed.
    pub edit: Option<String>,
    /// The live agent (`claude`, `codex`), or the one whose resume line waits
    /// at the prompt (`Pane::restored_agent()`), then with `resumable`.
    pub agent: Option<String>,
    pub resumable: bool,
    pub process: Option<String>,
    pub cwd: String,
    pub bookmarked: bool,
    pub focused: bool,
    pub preview: Option<PromptPreview>,
}

/// The reads of one `Tab` the model needs.
#[derive(Clone, Debug, Default)]
pub struct TabFacts {
    pub tab_idx: usize,
    pub tab_id: TabId,
    pub title: String,
    pub color: Option<usize>,
    pub active: bool,
    pub collapsed: bool,
    pub renaming: bool,
    /// Panes column by column, in sidebar order.
    pub panes: Vec<PaneFacts>,
}

/// The tile title, KovaLink's `paneLabel`: the agent session name, else the
/// pane's own title (custom or app-set) unless the shell put a directory there,
/// else the agent, else the foreground process, else `Shell`. A pane is never
/// titled by its directory: that is the subtitle's job.
pub fn tile_title(
    session_name: Option<&str>,
    custom_title: Option<&str>,
    osc_title: Option<&str>,
    agent: Option<&str>,
    process: Option<&str>,
    cwd: &str,
) -> String {
    if let Some(name) = pane_name(session_name, custom_title, osc_title, cwd) {
        return name;
    }
    if let Some(what) = present(agent).or_else(|| present(process)) {
        return what.to_string();
    }
    "Shell".to_string()
}

/// The group header of a tab without a custom name: the focused pane's name
/// when it has one, else its project (a tab is naturally named after its
/// directory, and that is what the phone groups by), else `shell`.
pub fn header_title(p: &PaneFacts) -> String {
    pane_name(p.session_name.as_deref(), p.custom_title.as_deref(), p.osc_title.as_deref(), &p.cwd)
        .or_else(|| Some(project_name(&p.cwd)).filter(|n| !n.is_empty()))
        .unwrap_or_else(|| "shell".to_string())
}

/// What names the pane, if anything: the session name, else its custom or
/// app-set title unless the shell put a directory there.
fn pane_name(session_name: Option<&str>, custom_title: Option<&str>, osc_title: Option<&str>, cwd: &str) -> Option<String> {
    present(session_name)
        .or_else(|| present(custom_title).or_else(|| present(osc_title)).filter(|t| !names_directory(t, cwd)))
        .map(str::to_string)
}

fn present(t: Option<&str>) -> Option<&str> {
    t.map(str::trim).filter(|t| !t.is_empty())
}

/// Whether a title is a directory rather than a name: the cwd or its basename,
/// a path (`/`, `~`, zsh's head-cut `..jects/Acme`), or a shell prompt
/// title (`user@host:~/dir`).
fn names_directory(title: &str, cwd: &str) -> bool {
    let tail = title.rsplit(':').next().unwrap_or(title).trim_start();
    title == cwd
        || (!cwd.is_empty() && title == project_name(cwd))
        || title.contains('/')
        || tail.starts_with('~')
        || title.starts_with("..")
}

/// KovaLink's `projectName`: the last path segment, the path itself when it
/// has none.
pub fn project_name(cwd: &str) -> String {
    let base = cwd.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    if base.is_empty() { cwd.to_string() } else { base.to_string() }
}

/// The subtitle, KovaLink's: `project · agent`, the agent left out when it is
/// already the title or when it is `claude` (the default agent here: naming
/// it is noise), and the project alone on a plain shell.
pub fn subtitle(project: &str, agent: Option<&str>, title: &str) -> String {
    match agent {
        Some("claude") => project.to_string(),
        Some(agent) if agent != title && !project.is_empty() => format!("{project} \u{b7} {agent}"),
        Some(agent) if agent != title => agent.to_string(),
        _ => project.to_string(),
    }
}

/// An errored tile's subtitle: the kind word, then what the provider said.
fn error_subtitle(kind: &str, text: &str) -> String {
    if text.is_empty() {
        return kind.to_string();
    }
    format!("{kind} \u{b7} {text}")
}

impl TileVm {
    pub fn from_facts(f: &PaneFacts) -> Self {
        let state = TileState::from_flags(f.flags);
        let mut question = None;
        let mut detail = None;
        let mut summary = None;
        match (state, f.preview.as_ref()) {
            (TileState::Awaiting, Some(PromptPreview::Permission { header, question: q, detail: d, .. })) => {
                question = Some(q.clone());
                detail = Some(d.clone().unwrap_or_else(|| header.clone()));
            }
            (TileState::Unread(_), Some(PromptPreview::TurnEnd { summary: s, .. })) if !s.is_empty() => {
                summary = Some(s.clone());
            }
            _ => {}
        }
        let title = tile_title(
            f.session_name.as_deref(),
            f.custom_title.as_deref(),
            f.osc_title.as_deref(),
            f.agent.as_deref(),
            f.process.as_deref(),
            &f.cwd,
        );
        let tooltip = match (state, f.preview.as_ref()) {
            (TileState::Error, Some(PromptPreview::ApiError { kind, text, .. })) => error_subtitle(kind, text),
            _ => subtitle(&project_name(&f.cwd), f.agent.as_deref(), &title),
        };
        TileVm {
            pane_id: f.pane_id,
            column: f.column,
            state,
            unread: f.flags.is_unread(),
            minimized: f.minimized,
            bare_shell: f.bare_shell && !f.resumable,
            resumable: f.bare_shell && f.resumable,
            title: f.edit.clone().unwrap_or(title),
            tooltip,
            agent: f.agent.clone(),
            bookmarked: f.bookmarked,
            focused: f.focused,
            renaming: f.edit.is_some(),
            question,
            detail,
            summary,
            waiting: None,
        }
    }

    /// The tile's right column: the state word while it works, nothing
    /// otherwise. An awaiting pane says `waiting 4m · Answer` on its subtitle
    /// instead (`waiting`), so its meta column stays empty.
    pub fn meta(&self) -> Option<String> {
        // A minimized pane is out of the way: no state word on its row.
        if self.minimized {
            return None;
        }
        match self.state {
            TileState::Working | TileState::Starting => Some(self.state.chip().to_string()),
            _ => None,
        }
    }
}

/// The panes of a window in the order Cmd+J and the pill walk them, each
/// with whether it is unread: the tabs in the sidebar's display order
/// (`sort`), the panes in tab order, minimized panes left out (never a
/// landing spot: the user folded them).
pub fn pane_order(tabs: &[TabFacts], sort: SidebarSort) -> Vec<(PaneId, bool)> {
    let tab_states: Vec<TileState> = tabs
        .iter()
        .map(|t| TileState::most_urgent(t.panes.iter().map(|p| TileState::from_flags(p.flags))))
        .collect();
    display_order(sort, &tab_states)
        .into_iter()
        .flat_map(|ti| tabs[ti].panes.iter())
        .filter(|p| !p.minimized)
        .map(|p| (p.pane_id, p.flags.is_unread()))
        .collect()
}

impl SidebarModel {
    /// Assemble the model. `unread` is the unread count across every window
    /// and `needs` this tick's need items: together they decide the pill.
    pub fn build(
        tabs: &[TabFacts],
        sort: SidebarSort,
        unread: usize,
        view: SidebarList,
        routines: Option<Vec<RoutineVm>>,
        needs: &[NeedItem],
    ) -> Self {
        let tab_states: Vec<TileState> = tabs
            .iter()
            .map(|t| TileState::most_urgent(t.panes.iter().map(|p| TileState::from_flags(p.flags))))
            .collect();
        let mut waiting = 0;
        let mut working = 0;
        let mut idle_here = 0;
        let mut total_panes = 0;
        let mut agents_working = 0;
        for tab in tabs {
            for p in &tab.panes {
                total_panes += 1;
                agents_working += u32::from(p.flags.working);
                match TileState::from_flags(p.flags) {
                    TileState::Awaiting => waiting += 1,
                    TileState::Working | TileState::Starting => working += 1,
                    TileState::Idle => idle_here += 1,
                    _ => {}
                }
            }
        }
        let groups = display_order(sort, &tab_states)
            .into_iter()
            .map(|ti| {
                let tab = &tabs[ti];
                let tiles = if tab.collapsed {
                    Vec::new()
                } else {
                    tab.panes.iter().map(TileVm::from_facts).collect()
                };
                GroupVm {
                    tab_idx: tab.tab_idx,
                    tab_id: tab.tab_id,
                    title: tab.title.clone(),
                    color: tab.color,
                    active: tab.active,
                    collapsed: tab.collapsed,
                    renaming: tab.renaming,
                    summary: CollapsedSummary::of(tab.panes.iter().map(|p| TileState::from_flags(p.flags))),
                    tiles,
                }
            })
            .collect();
        SidebarModel {
            view,
            routines_loaded: routines.is_some(),
            routines: routines.unwrap_or_default(),
            // The badge comes from the needs model, which needs the live
            // panes and the board: `sync_sidebar` sets it right after.
            routine_failed: false,
            agents_working,
            summary: summary_runs(waiting, working, idle_here),
            sort,
            pill: next_pill(needs, unread),
            groups,
            needs: Vec::new(),
            cockpit: None,
            show_hint: total_panes < 3,
            sched: SchedState::default(),
            sched_meta: Vec::new(),
            agents: AgentsVm::default(),
            agent_state: AgentsState::default(),
            meta_right: None,
        }
    }

    /// Fill the need rows and, for every pane with an open ask or a
    /// permission prompt, the age its tile's subtitle shows. `items` is the
    /// tick's `needs::need_items`: the same list the badges and Next read.
    pub fn apply_needs(&mut self, items: &[NeedItem], now: u64) {
        self.needs = need_rows(items, now);
        for group in &mut self.groups {
            for tile in &mut group.tiles {
                tile.waiting = items
                    .iter()
                    .find(|item| {
                        item.pane_id == Some(u64::from(tile.pane_id))
                            && matches!(item.kind, NeedKind::Ask | NeedKind::Permission)
                    })
                    .map(|item| format_age(now.saturating_sub(item.since)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_routine_shows_its_schedule_until_something_stops_it_from_running() {
        use crate::routines::{LastRun, LastStatus, When};
        let now = When::parse("2026-09-26T10:20").expect("a clock");
        let ok = crate::routines::Routine {
            name: "alpha-digest".into(),
            schedule: "daily 10:02".into(),
            schedule_words: "daily 10:02".into(),
            loaded: true,
            next_run: Some(When::parse("2026-10-03T10:20").expect("a date")),
            last: Some(LastRun {
                start: None,
                end: Some(When::parse("2026-09-26T10:04:34").expect("a date")),
                code: 0,
            }),
            ..Default::default()
        };
        let vm = RoutineVm::at(&ok, false, now);
        assert!(!vm.schedule_words.is_empty());
        assert_eq!(vm.schedule_words, "daily 10:02");
        assert_eq!(vm.last_status, LastStatus::Ok);
        assert_eq!(vm.ago_words, "16 min ago");
        assert_eq!(vm.group, crate::routines::Group::ThisWeek);
        assert_eq!(vm.next_words, "Sat 10:20");
        assert_eq!(vm.in_words, "in 7 d");
        assert_eq!(vm.next_at, ok.next_run);

        // A plist launchd never loaded: its group still follows the schedule,
        // and the row's second line keeps the schedule it was given.
        let never = crate::routines::Routine { loaded: false, ..ok.clone() };
        assert_eq!(never.problem(), Some("not loaded"));
        assert_eq!(RoutineVm::at(&never, false, now).schedule_words, "daily 10:02");

        // A non-zero exit is worth surfacing next to the date.
        let failed = crate::routines::Routine {
            last: Some(LastRun { start: None, end: Some(now), code: 2 }),
            ..ok.clone()
        };
        let vm = RoutineVm::at(&failed, false, now);
        assert_eq!(vm.last_status, LastStatus::Failed(2));
        // It stays in its schedule's group: the Cockpit is where it asks.
        assert_ne!(vm.group, crate::routines::Group::Off);

        // A run still going has no end: it is running, not failed, and the
        // row counts from its start.
        let running = crate::routines::Routine {
            running: true,
            last: Some(LastRun { start: Some(When::parse("2026-09-26T10:15").expect("a date")), end: None, code: 0 }),
            ..ok.clone()
        };
        let vm = RoutineVm::at(&running, false, now);
        assert_eq!(vm.last_status, LastStatus::Running);
        assert_eq!(vm.ago_words, "5 min ago");

        // Closed, the body is not even carried: the model stays small and
        // cheap to compare on every tick.
        let fat = crate::routines::Routine { cwd: "/Users/x/project".into(), ..ok.clone() };
        assert_eq!(RoutineVm::at(&fat, false, now).cwd, "");
        assert_eq!(RoutineVm::at(&fat, true, now).cwd, "/Users/x/project");
    }

    #[test]
    fn the_sidebar_opens_on_the_panes_list() {
        assert_eq!(SidebarList::default(), SidebarList::Panes);
    }

    #[test]
    fn every_list_names_itself_in_the_chrome() {
        let titles: Vec<&str> = [SidebarList::Panes, SidebarList::Agents, SidebarList::Scheduled]
            .iter()
            .map(|l| l.title())
            .collect();
        assert_eq!(titles, ["Panes", "Agents", "Scheduled"]);
    }

    fn pane(id: PaneId, flags: PaneFlags) -> PaneFacts {
        PaneFacts {
            pane_id: id,
            session_name: Some(format!("pane {id}")),
            agent: Some("claude".into()),
            cwd: "/Users/me/link".into(),
            flags,
            ..PaneFacts::default()
        }
    }

    fn tab(idx: usize, panes: Vec<PaneFacts>) -> TabFacts {
        TabFacts { tab_idx: idx, tab_id: 100 + idx as u32, title: format!("tab {idx}"), panes, ..TabFacts::default() }
    }

    #[test]
    fn a_tile_carries_the_secondary_line_and_the_preview_of_its_state() {
        let mut f = pane(1, PaneFlags { permission_prompt: true, ..PaneFlags::default() });
        f.preview = Some(PromptPreview::Permission {
            header: "Bash command".into(),
            question: "Run it?".into(),
            detail: None,
            since: 1000,
            seen: false,
        });
        let t = TileVm::from_facts(&f);
        assert_eq!(t.state, TileState::Awaiting);
        assert_eq!(t.title, "pane 1");
        assert_eq!(t.tooltip, "link");
        assert_eq!(t.question.as_deref(), Some("Run it?"));
        // No detail: the header stands in.
        assert_eq!(t.detail.as_deref(), Some("Bash command"));
        // The age is the need block's business (`apply_needs`), and the meta
        // column is empty: the subtitle says `waiting 4m · Answer`.
        assert_eq!(t.waiting, None);
        assert_eq!(t.meta(), None);
        assert_eq!(t.summary, None);

        // Working tiles say so; idle ones have nothing to add.
        let working = TileVm::from_facts(&pane(4, PaneFlags { working: true, ..PaneFlags::default() }));
        assert_eq!(working.meta().as_deref(), Some("working"));
        let idle = TileVm::from_facts(&pane(5, PaneFlags { idle_agent: true, ..PaneFlags::default() }));
        assert_eq!(idle.meta(), None);
        // A minimized pane keeps quiet, starting or not.
        let mut folded = pane(6, PaneFlags { starting: true, ..PaneFlags::default() });
        folded.minimized = true;
        assert_eq!(TileVm::from_facts(&folded).meta(), None);

        // The turn-end summary only shows on an unread tile.
        let mut f = pane(2, PaneFlags { turn_end_unseen: true, ..PaneFlags::default() });
        f.preview = Some(PromptPreview::TurnEnd { summary: "Pushed the copy".into(), seen: false });
        let t = TileVm::from_facts(&f);
        assert_eq!(t.state, TileState::Unread(UnreadKind::Done));
        assert_eq!(t.summary.as_deref(), Some("Pushed the copy"));
        assert_eq!(t.waiting, None);
        let mut f = pane(3, PaneFlags { turn_end_unseen: true, seen: true, idle_agent: true, ..PaneFlags::default() });
        f.preview = Some(PromptPreview::TurnEnd { summary: "Pushed the copy".into(), seen: true });
        let t = TileVm::from_facts(&f);
        assert_eq!(t.state, TileState::Idle);
        assert_eq!(t.summary, None);
    }

    #[test]
    fn an_errored_tile_names_the_error_and_what_the_provider_said() {
        let mut f = pane(6, PaneFlags { error: true, idle_agent: true, ..PaneFlags::default() });
        f.preview = Some(PromptPreview::ApiError {
            kind: "Limit reached".into(),
            text: "You've hit your session limit".into(),
            since: 1,
            seen: false,
        });
        let t = TileVm::from_facts(&f);
        assert_eq!(t.state, TileState::Error);
        assert_eq!(t.title, "pane 6");
        assert_eq!(t.tooltip, "Limit reached · You've hit your session limit");
        assert_eq!(t.meta(), None);
        // A turn in flight hides it, subtitle and all.
        let mut busy = f.clone();
        busy.flags.working = true;
        let t = TileVm::from_facts(&busy);
        assert_eq!(t.state, TileState::Working);
        assert_eq!(t.tooltip, "link");
    }

    #[test]
    fn the_session_name_wins_and_a_directory_never_titles_a_tile() {
        let cwd = "/Users/me/Projects/Acme";
        // `/rename` beats everything, the sticky title included.
        assert_eq!(tile_title(Some("Fix the login"), Some("claude"), Some("Claude Code"), Some("claude"), None, cwd), "Fix the login");
        assert_eq!(tile_title(Some("  "), Some("my pane"), None, Some("claude"), None, cwd), "my pane");
        // The shell's OSC 1 titles, head-cut cwd and prompt alike, are not names.
        assert_eq!(tile_title(None, Some("..jects/Acme"), Some("me@mac:~/Projects/Acme"), Some("claude"), None, cwd), "claude");
        assert_eq!(tile_title(None, Some("~/link"), None, None, None, "/Users/me/link"), "Shell");
        assert_eq!(tile_title(None, Some("~"), None, None, None, "/Users/me"), "Shell");
        assert_eq!(tile_title(None, Some("Acme"), None, None, None, cwd), "Shell");
        assert_eq!(tile_title(None, None, Some("me@mac:~"), None, None, "/Users/me"), "Shell");
        assert_eq!(tile_title(None, None, Some(cwd), None, None, cwd), "Shell");
        // The app's own title, then the process, then the fallback.
        assert_eq!(tile_title(None, None, Some("Claude Code"), Some("claude"), None, cwd), "Claude Code");
        assert_eq!(tile_title(None, Some("claude"), Some("Claude Code"), Some("claude"), None, cwd), "claude");
        assert_eq!(tile_title(None, None, None, None, Some("nvim"), cwd), "nvim");
        assert_eq!(tile_title(None, None, None, None, None, cwd), "Shell");
        assert_eq!(tile_title(None, None, None, None, None, ""), "Shell");
    }

    #[test]
    fn a_header_without_a_tab_name_takes_the_pane_name_then_its_project() {
        let mut p = PaneFacts { cwd: "/Users/me/Projects/Home".into(), custom_title: Some("..jects/Home".into()), ..PaneFacts::default() };
        assert_eq!(header_title(&p), "Home");
        p.session_name = Some("Taxes".into());
        assert_eq!(header_title(&p), "Taxes");
        let empty = PaneFacts::default();
        assert_eq!(header_title(&empty), "shell");
    }

    #[test]
    fn the_subtitle_is_the_project_then_the_agent_unless_it_is_the_title() {
        assert_eq!(project_name("/Users/me/Projects/Acme"), "Acme");
        assert_eq!(project_name("/Users/me/link/"), "link");
        assert_eq!(project_name("/"), "/");
        assert_eq!(project_name(""), "");
        // `claude` is the default agent: the subtitle says the project only.
        assert_eq!(subtitle("Acme", Some("claude"), "Fix the login"), "Acme");
        assert_eq!(subtitle("Acme", Some("claude"), "claude"), "Acme");
        assert_eq!(subtitle("Acme", Some("codex"), "Shell"), "Acme \u{b7} codex");
        assert_eq!(subtitle("Acme", None, "Shell"), "Acme");
        assert_eq!(subtitle("", Some("codex"), "Shell"), "codex");
        assert_eq!(subtitle("", None, "Shell"), "");
    }

    #[test]
    fn a_restored_session_resumes_and_a_bare_shell_starts_claude() {
        // Restored: the resume line waits at the prompt, no agent yet.
        let restored = PaneFacts {
            pane_id: 1,
            bare_shell: true,
            resumable: true,
            agent: Some("claude".into()),
            custom_title: Some("..jects/Acme".into()),
            osc_title: Some("me@mac:~/Projects/Acme".into()),
            cwd: "/Users/me/Projects/Acme".into(),
            ..PaneFacts::default()
        };
        let t = TileVm::from_facts(&restored);
        assert_eq!(t.state, TileState::Shell);
        assert!(t.resumable);
        assert!(!t.bare_shell);
        assert_eq!(t.meta(), None);
        assert_eq!(t.title, "claude");
        assert_eq!(t.tooltip, "Acme");

        // A plain shell: `shell` chip and `Start Claude`.
        let bare = PaneFacts { pane_id: 2, bare_shell: true, cwd: "/Users/me/link".into(), ..PaneFacts::default() };
        let t = TileVm::from_facts(&bare);
        assert!(t.bare_shell);
        assert!(!t.resumable);
        assert_eq!(t.meta(), None);
        assert_eq!(t.title, "Shell");
        assert_eq!(t.tooltip, "link");

        // A live idle session keeps its state chip.
        let live = PaneFacts {
            pane_id: 3,
            flags: PaneFlags { idle_agent: true, ..PaneFlags::default() },
            session_name: Some("Sidebar identity".into()),
            agent: Some("claude".into()),
            cwd: "/Users/me/kova".into(),
            ..PaneFacts::default()
        };
        let t = TileVm::from_facts(&live);
        assert_eq!(t.meta(), None);
        assert_eq!(t.title, "Sidebar identity");
        assert_eq!(t.tooltip, "kova");
        assert!(!t.bare_shell && !t.resumable);

        // Renaming: the edit buffer is the title.
        let mut renaming = live.clone();
        renaming.edit = Some("Side\u{258f}".into());
        let t = TileVm::from_facts(&renaming);
        assert_eq!(t.title, "Side\u{258f}");
        assert!(t.renaming);
    }

    #[test]
    fn the_model_counts_the_window_and_folds_collapsed_groups() {
        let mut folded = tab(1, vec![pane(3, PaneFlags { working: true, ..PaneFlags::default() })]);
        folded.collapsed = true;
        let tabs = vec![
            tab(0, vec![pane(1, PaneFlags { permission_prompt: true, ..PaneFlags::default() }), pane(2, PaneFlags { idle_agent: true, ..PaneFlags::default() })]),
            folded,
        ];
        let m = SidebarModel::build(&tabs, SidebarSort::Kova, 2, SidebarList::default(), None, &[]);
        let text: String = m.summary.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text, "1 waiting \u{b7} 1 working \u{b7} 1 idle");
        // A folded group still counts: the Agents badge is every working pane.
        assert_eq!(m.agents_working, 1);
        assert_eq!(m.pill, NextPill::Next(2));
        assert_eq!(m.groups.len(), 2);
        assert_eq!(m.groups[0].tiles.len(), 2);
        assert!(m.groups[1].tiles.is_empty());
        assert_eq!(m.groups[1].summary, CollapsedSummary { awaiting: 0, working: 1, count: 1 });
        assert_eq!(m.groups[1].tab_id, 101);
        // Three panes: no hint.
        assert!(!m.show_hint);
        assert_eq!(m.groups[0].tiles[1].state, TileState::Idle);
    }

    #[test]
    fn activity_sort_reorders_the_groups_and_keeps_tab_indices() {
        let tabs = vec![
            tab(0, vec![pane(1, PaneFlags::default())]),
            tab(1, vec![pane(2, PaneFlags { permission_prompt: true, ..PaneFlags::default() })]),
        ];
        let m = SidebarModel::build(&tabs, SidebarSort::Activity, 0, SidebarList::default(), None, &[]);
        assert_eq!(m.groups.iter().map(|g| g.tab_idx).collect::<Vec<_>>(), vec![1, 0]);
        assert_eq!(m.pill, NextPill::Nothing);
        assert!(m.show_hint);
        let same = SidebarModel::build(&tabs, SidebarSort::Activity, 0, SidebarList::default(), None, &[]);
        assert_eq!(m, same);
    }

    #[test]
    fn the_unread_walk_follows_the_display_order_and_skips_minimized_panes() {
        let unread = PaneFlags { completion: true, ..PaneFlags::default() };
        let mut folded_away = pane(4, unread);
        folded_away.minimized = true;
        let tabs = vec![
            tab(0, vec![pane(1, PaneFlags::default()), pane(2, unread), folded_away]),
            tab(1, vec![pane(5, PaneFlags { permission_prompt: true, prompt_unseen: true, ..PaneFlags::default() }), pane(6, PaneFlags { manual: true, seen: true, ..PaneFlags::default() })]),
            tab(2, vec![pane(7, PaneFlags { idle_agent: true, ..PaneFlags::default() })]),
        ];
        // Tab order: every pane but the minimized one, unread where
        // something is new or marked; the idle one is read.
        let unread = |order: Vec<(PaneId, bool)>| order.into_iter().filter(|&(_, u)| u).map(|(id, _)| id).collect::<Vec<_>>();
        let kova = pane_order(&tabs, SidebarSort::Kova);
        assert_eq!(kova.iter().map(|&(id, _)| id).collect::<Vec<_>>(), vec![1, 2, 5, 6, 7]);
        assert_eq!(unread(kova), vec![2, 5, 6]);
        // Activity order: the awaiting tab first.
        assert_eq!(unread(pane_order(&tabs, SidebarSort::Activity)), vec![5, 6, 2]);
        let m = SidebarModel::build(&tabs, SidebarSort::Kova, 3, SidebarList::default(), None, &[]);
        assert!(m.groups[0].tiles[1].unread && !m.groups[0].tiles[0].unread);
        assert!(m.groups[1].tiles[1].unread);
    }

    #[test]
    fn the_next_pill_counts_what_waits_on_you_before_unread_output() {
        let tabs = vec![tab(0, vec![pane(1, PaneFlags::default())])];
        let item = |kind: NeedKind, pane: u64| NeedItem {
            kind,
            key: format!("pane:{pane}"),
            pane_id: Some(pane),
            since: pane,
            title: String::new(),
            detail: String::new(),
        };
        // Two asks waiting and unread output: the asks win, their own count.
        let needs = vec![item(NeedKind::Ask, 1), item(NeedKind::Ask, 2)];
        let m = SidebarModel::build(&tabs, SidebarSort::Kova, 5, SidebarList::default(), None, &needs);
        assert_eq!(m.pill, NextPill::Next(2));
        // One permission prompt alone is already a stop.
        let needs = vec![item(NeedKind::Permission, 3)];
        assert_eq!(next_pill(&needs, 0), NextPill::Next(1));
        // An error has a target but nobody to answer: still a stop, never 0.
        let needs = vec![item(NeedKind::Error, 4)];
        assert_eq!(next_pill(&needs, 0), NextPill::Next(1));
        // Nothing waits on you: unread output, as before.
        assert_eq!(next_pill(&[], 4), NextPill::Next(4));
        // Neither: the pill is not a button.
        assert_eq!(next_pill(&[], 0), NextPill::Nothing);
        // A failed routine has no pane to land on: not a pill stop.
        let routine = NeedItem {
            kind: NeedKind::FailedRoutine,
            key: "routine:nightly".into(),
            pane_id: None,
            since: 1,
            title: "nightly".into(),
            detail: "Routine failed".into(),
        };
        assert_eq!(next_pill(&[routine], 0), NextPill::Nothing);
    }
}
