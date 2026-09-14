//! The sidebar's view model: a plain, comparable description of what the
//! AppKit sidebar shows for one window, built once per tick from the tabs.
//! The view re-lays itself out only when the model changed. The reads from
//! `Pane` happen in `sidebar_ui.rs` (`PaneFacts`, `TabFacts`); everything from
//! the facts to the model is pure and tested here.

use super::sidebar::{
    display_order, format_age, secondary_line, summary_runs, CollapsedSummary, NextPill, PaneFlags,
    SidebarSort, SummaryRun, TileState, AGING_SECS,
};
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
    pub minimized: bool,
    pub bare_shell: bool,
    /// Pane title, already the edit buffer while renaming.
    pub title: String,
    /// `claude · ~/cwd`.
    pub secondary: String,
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
    pub title: String,
    pub renaming: bool,
    pub agent: Option<String>,
    pub process: Option<String>,
    pub cwd_short: String,
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

impl TileVm {
    pub fn from_facts(f: &PaneFacts, now: u64) -> Self {
        let state = TileState::from_flags(f.flags);
        let mut question = None;
        let mut detail = None;
        let mut summary = None;
        let mut age = None;
        match (state, f.preview.as_ref()) {
            (TileState::Awaiting, Some(PromptPreview::Permission { header, question: q, detail: d, since })) => {
                question = Some(q.clone());
                detail = Some(d.clone().unwrap_or_else(|| header.clone()));
                let secs = now.saturating_sub(*since);
                age = Some((format_age(secs), secs >= AGING_SECS));
            }
            (TileState::Unread { .. }, Some(PromptPreview::TurnEnd { summary: s, .. })) if !s.is_empty() => {
                summary = Some(s.clone());
            }
            _ => {}
        }
        TileVm {
            pane_id: f.pane_id,
            column: f.column,
            state,
            minimized: f.minimized,
            bare_shell: f.bare_shell,
            title: f.title.clone(),
            secondary: secondary_line(f.agent.as_deref(), f.process.as_deref(), &f.cwd_short),
            bookmarked: f.bookmarked,
            focused: f.focused,
            renaming: f.renaming,
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
}

impl SidebarModel {
    /// Assemble the model. `unread` and `idle` are Cmd+J's tiers across every
    /// window (the pill), `flashing` the caught-up flash.
    pub fn build(tabs: &[TabFacts], sort: SidebarSort, unread: usize, idle: usize, flashing: bool, now: u64) -> Self {
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
            pill: NextPill::of(unread, idle, flashing),
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
            title: format!("pane {id}"),
            agent: Some("claude".into()),
            cwd_short: "~/link".into(),
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
        });
        let t = TileVm::from_facts(&f, 1000 + 700);
        assert_eq!(t.state, TileState::Awaiting);
        assert!(t.awaiting());
        assert_eq!(t.secondary, "claude \u{b7} ~/link");
        assert_eq!(t.question.as_deref(), Some("Run it?"));
        // No detail: the header stands in.
        assert_eq!(t.detail.as_deref(), Some("Bash command"));
        assert_eq!(t.age, Some(("11m".into(), true)));
        assert_eq!(t.summary, None);

        // The turn-end summary only shows on an unread tile.
        let mut f = pane(2, PaneFlags { turn_end_unseen: true, ..PaneFlags::default() });
        f.preview = Some(PromptPreview::TurnEnd { summary: "Pushed the copy".into(), seen: false });
        let t = TileVm::from_facts(&f, 0);
        assert_eq!(t.state, TileState::Unread { bell: false });
        assert_eq!(t.summary.as_deref(), Some("Pushed the copy"));
        assert_eq!(t.age, None);
        let mut f = pane(3, PaneFlags { turn_end_unseen: true, seen: true, idle_agent: true, ..PaneFlags::default() });
        f.preview = Some(PromptPreview::TurnEnd { summary: "Pushed the copy".into(), seen: true });
        let t = TileVm::from_facts(&f, 0);
        assert_eq!(t.state, TileState::Idle);
        assert_eq!(t.summary, None);
    }

    #[test]
    fn the_model_counts_the_window_and_folds_collapsed_groups() {
        let mut folded = tab(1, vec![pane(3, PaneFlags { working: true, ..PaneFlags::default() })]);
        folded.collapsed = true;
        let tabs = vec![
            tab(0, vec![pane(1, PaneFlags { permission_prompt: true, ..PaneFlags::default() }), pane(2, PaneFlags { idle_agent: true, ..PaneFlags::default() })]),
            folded,
        ];
        let m = SidebarModel::build(&tabs, SidebarSort::Kova, 2, 1, false, 0);
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
        let m = SidebarModel::build(&tabs, SidebarSort::Activity, 0, 0, true, 0);
        assert_eq!(m.groups.iter().map(|g| g.tab_idx).collect::<Vec<_>>(), vec![1, 0]);
        assert_eq!(m.pill, NextPill::CaughtUp);
        assert!(m.show_hint);
        let same = SidebarModel::build(&tabs, SidebarSort::Activity, 0, 0, true, 0);
        assert_eq!(m, same);
    }
}
