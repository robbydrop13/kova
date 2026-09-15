//! The sidebar's view model: a plain, comparable description of what the
//! AppKit sidebar shows for one window, built once per tick from the tabs.
//! The view re-lays itself out only when the model changed. The reads from
//! `Pane` happen in `sidebar_ui.rs` (`PaneFacts`, `TabFacts`); everything from
//! the facts to the model is pure and tested here.

use super::sidebar::{
    display_order, format_age, summary_runs, CollapsedSummary, NextPill, PaneFlags, SidebarSort,
    SummaryRun, TileState, AGING_SECS,
};
#[cfg(test)]
use super::sidebar::UnreadKind;
use crate::pane::{PaneId, TabId};
use crate::prompt_preview::PromptPreview;

/// Everything the sidebar shows for one window.
#[derive(Clone, Debug, PartialEq)]
pub struct SidebarModel {
    /// `2 waiting · 3 working · 4 idle`, one coloured run at a time.
    pub summary: Vec<(String, SummaryRun)>,
    pub sort: SidebarSort,
    pub pill: NextPill,
    /// The groups in display order.
    pub groups: Vec<GroupVm>,
    /// Fewer than three panes in the window: show the `⌘T new tab · ⌘D split` hint.
    pub show_hint: bool,
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
    /// `project · claude`, the agent left out when it is the title.
    pub secondary: String,
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
    /// Awaiting tile: `4m` and whether it is old enough to go red.
    pub age: Option<(String, bool)>,
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
/// already the title.
pub fn subtitle(project: &str, agent: Option<&str>, title: &str) -> String {
    match agent {
        Some(agent) if agent != title && !project.is_empty() => format!("{project} \u{b7} {agent}"),
        Some(agent) if agent != title => agent.to_string(),
        _ => project.to_string(),
    }
}

impl TileVm {
    pub fn from_facts(f: &PaneFacts, now: u64) -> Self {
        let state = TileState::from_flags(f.flags);
        let mut question = None;
        let mut detail = None;
        let mut summary = None;
        let mut age = None;
        match (state, f.preview.as_ref()) {
            (TileState::Awaiting, Some(PromptPreview::Permission { header, question: q, detail: d, since, .. })) => {
                question = Some(q.clone());
                detail = Some(d.clone().unwrap_or_else(|| header.clone()));
                let secs = now.saturating_sub(*since);
                age = Some((format_age(secs), secs >= AGING_SECS));
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
        let secondary = subtitle(&project_name(&f.cwd), f.agent.as_deref(), &title);
        TileVm {
            pane_id: f.pane_id,
            column: f.column,
            state,
            unread: f.flags.is_unread(),
            minimized: f.minimized,
            bare_shell: f.bare_shell && !f.resumable,
            resumable: f.bare_shell && f.resumable,
            title: f.edit.clone().unwrap_or(title),
            secondary,
            agent: f.agent.clone(),
            bookmarked: f.bookmarked,
            focused: f.focused,
            renaming: f.edit.is_some(),
            question,
            detail,
            summary,
            age,
        }
    }

    /// Whether the tile is the tall awaiting card.
    pub fn awaiting(&self) -> bool {
        self.state == TileState::Awaiting
    }

    /// The chip: the state word, except that a shell holding an agent's
    /// resume line names that agent (`claude`, `codex`) rather than `shell`.
    pub fn chip(&self) -> String {
        match (self.state, self.resumable, self.agent.as_deref()) {
            (TileState::Shell, true, Some(agent)) => agent.to_string(),
            _ => self.state.chip().to_string(),
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
    /// (the pill).
    pub fn build(tabs: &[TabFacts], sort: SidebarSort, unread: usize, now: u64) -> Self {
        let tab_states: Vec<TileState> = tabs
            .iter()
            .map(|t| TileState::most_urgent(t.panes.iter().map(|p| TileState::from_flags(p.flags))))
            .collect();
        let mut waiting = 0;
        let mut working = 0;
        let mut idle_here = 0;
        let mut total_panes = 0;
        for tab in tabs {
            for p in &tab.panes {
                total_panes += 1;
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
                    tab.panes.iter().map(|p| TileVm::from_facts(p, now)).collect()
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
            summary: summary_runs(waiting, working, idle_here),
            sort,
            pill: NextPill::of(unread),
            groups,
            show_hint: total_panes < 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let t = TileVm::from_facts(&f, 1000 + 700);
        assert_eq!(t.state, TileState::Awaiting);
        assert!(t.awaiting());
        assert_eq!(t.title, "pane 1");
        assert_eq!(t.secondary, "link \u{b7} claude");
        assert_eq!(t.question.as_deref(), Some("Run it?"));
        // No detail: the header stands in.
        assert_eq!(t.detail.as_deref(), Some("Bash command"));
        assert_eq!(t.age, Some(("11m".into(), true)));
        assert_eq!(t.summary, None);

        // The turn-end summary only shows on an unread tile.
        let mut f = pane(2, PaneFlags { turn_end_unseen: true, ..PaneFlags::default() });
        f.preview = Some(PromptPreview::TurnEnd { summary: "Pushed the copy".into(), seen: false });
        let t = TileVm::from_facts(&f, 0);
        assert_eq!(t.state, TileState::Unread(UnreadKind::Done));
        assert_eq!(t.summary.as_deref(), Some("Pushed the copy"));
        assert_eq!(t.age, None);
        let mut f = pane(3, PaneFlags { turn_end_unseen: true, seen: true, idle_agent: true, ..PaneFlags::default() });
        f.preview = Some(PromptPreview::TurnEnd { summary: "Pushed the copy".into(), seen: true });
        let t = TileVm::from_facts(&f, 0);
        assert_eq!(t.state, TileState::Idle);
        assert_eq!(t.summary, None);
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
        assert_eq!(subtitle("Acme", Some("claude"), "Fix the login"), "Acme \u{b7} claude");
        assert_eq!(subtitle("Acme", Some("claude"), "claude"), "Acme");
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
        let t = TileVm::from_facts(&restored, 0);
        assert_eq!(t.state, TileState::Shell);
        assert!(t.resumable);
        assert!(!t.bare_shell);
        assert_eq!(t.chip(), "claude");
        assert_eq!(t.title, "claude");
        assert_eq!(t.secondary, "Acme");

        // A plain shell: `shell` chip and `Start Claude`.
        let bare = PaneFacts { pane_id: 2, bare_shell: true, cwd: "/Users/me/link".into(), ..PaneFacts::default() };
        let t = TileVm::from_facts(&bare, 0);
        assert!(t.bare_shell);
        assert!(!t.resumable);
        assert_eq!(t.chip(), "shell");
        assert_eq!(t.title, "Shell");
        assert_eq!(t.secondary, "link");

        // A live idle session keeps its state chip.
        let live = PaneFacts {
            pane_id: 3,
            flags: PaneFlags { idle_agent: true, ..PaneFlags::default() },
            session_name: Some("Sidebar identity".into()),
            agent: Some("claude".into()),
            cwd: "/Users/me/kova".into(),
            ..PaneFacts::default()
        };
        let t = TileVm::from_facts(&live, 0);
        assert_eq!(t.chip(), "idle");
        assert_eq!(t.title, "Sidebar identity");
        assert_eq!(t.secondary, "kova \u{b7} claude");
        assert!(!t.bare_shell && !t.resumable);

        // Renaming: the edit buffer is the title.
        let mut renaming = live.clone();
        renaming.edit = Some("Side\u{258f}".into());
        let t = TileVm::from_facts(&renaming, 0);
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
        let m = SidebarModel::build(&tabs, SidebarSort::Kova, 2, 0);
        let text: String = m.summary.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text, "1 waiting \u{b7} 1 working \u{b7} 1 idle");
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
        let m = SidebarModel::build(&tabs, SidebarSort::Activity, 0, 0);
        assert_eq!(m.groups.iter().map(|g| g.tab_idx).collect::<Vec<_>>(), vec![1, 0]);
        assert_eq!(m.pill, NextPill::Nothing);
        assert!(m.show_hint);
        let same = SidebarModel::build(&tabs, SidebarSort::Activity, 0, 0);
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
        let m = SidebarModel::build(&tabs, SidebarSort::Kova, 3, 0);
        assert!(m.groups[0].tiles[1].unread && !m.groups[0].tiles[0].unread);
        assert!(m.groups[1].tiles[1].unread);
    }
}
