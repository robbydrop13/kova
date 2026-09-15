//! The sidebar layout mode: the tabs of a window as collapsible groups down
//! the left edge, one tile per pane, instead of the strip across the top.
//! The look and the vocabulary are KovaLink's home screen: tab groups with a
//! colour bar, tiles with a state glyph and a chip, the summary line and the
//! Next pill, hover actions.
//!
//! This module holds the parts that need no window: the process-wide layout
//! setting and its persistence, the colour tokens, the state vocabulary
//! (`TileState`), the activity sort, the collapsed summary, the Next pill,
//! the pane drag arithmetic and the text rules. The view model lives in
//! `sidebar_model.rs`, the AppKit view and its layout in `sidebar_view.rs`.
//! See `docs/sidebar-spec.md`.

use std::sync::atomic::{AtomicU16, AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

use super::feather::Icon;
use crate::config::{clamp_sidebar_width, LayoutConfig, LayoutMode, LayoutPrefs};

// ---------------------------------------------------------------
// Process-wide layout setting
// ---------------------------------------------------------------
//
// The mode and the sidebar width are one setting for the whole app, not one
// per window: the View menu, the shortcut and the edge drag all change "how
// Kova shows tabs". Every window reads them on each frame.

static MODE: AtomicU8 = AtomicU8::new(0);
static WIDTH_PT: AtomicU16 = AtomicU16::new(280);

/// Adopt the layout the config resolved to (`[layout]` table plus the
/// `prefs.json` overrides). Called once at startup.
pub fn init(layout: &LayoutConfig) {
    MODE.store(mode_to_u8(layout.mode), Ordering::Relaxed);
    WIDTH_PT.store(clamp_sidebar_width(layout.sidebar_width), Ordering::Relaxed);
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

/// Sidebar width in points, separator included.
pub fn width_pt() -> u16 {
    WIDTH_PT.load(Ordering::Relaxed)
}

/// Set the width (snapped into range). Returns whether it changed. Not
/// persisted here: an edge drag calls this on every event and `persist`
/// once, on mouse up.
pub fn set_width_pt(pts: u16) -> bool {
    let pts = clamp_sidebar_width(pts);
    WIDTH_PT.swap(pts, Ordering::Relaxed) != pts
}

/// Write the runtime layout state to `prefs.json`.
pub fn persist() {
    LayoutPrefs { mode: Some(layout_mode()), sidebar_width: Some(width_pt()) }.save();
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
    pub const TEXT_ON_FILL: [f32; 3] = [1.0, 1.0, 1.0];
    pub const AWAITING: [f32; 3] = [1.000, 0.690, 0.125];
    pub const AWAITING_BG: [f32; 3] = [0.165, 0.122, 0.031];
    pub const WORKING: [f32; 3] = [0.220, 0.741, 0.973];
    pub const SUCCESS: [f32; 3] = [0.239, 0.839, 0.549];
    pub const ERROR: [f32; 3] = [1.000, 0.361, 0.361];
    pub const INTERRUPT: [f32; 3] = [1.000, 0.478, 0.478];
    pub const TAB_NONE: [f32; 3] = [0.486, 0.522, 0.576];
    pub const SEPARATOR: [f32; 3] = BORDER_SUBTLE;
    /// The white every translucent layer of the selected group is cut from.
    pub const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
    pub const BLACK: [f32; 3] = [0.0, 0.0, 0.0];
}

// ---------------------------------------------------------------
// Selected tab wash
// ---------------------------------------------------------------

/// Alpha of the tab colour at the top of a group's panel: the selected tab's
/// strength, and the other tabs'.
pub const WASH_SELECTED: f64 = 0.40;
pub const WASH_OTHER: f64 = 0.20;
/// Alpha of the white the selected group's tiles are filled with.
pub const SEL_TILE_ALPHA: f64 = 0.12;
/// The tile's lift under the mouse (a point less on the focused tile, which
/// is already the brightest), and the focused tile's lift.
pub const SEL_TILE_HOVER_LIFT: f64 = 0.04;
pub const SEL_TILE_FOCUS_LIFT: f64 = 0.10;

/// The vertical gradient washed over a group's panel: the tab colour at
/// `strength` at the top, fading to 45 % of it at 42 % of the height and
/// 14 % at the bottom. `(alpha, location)`, top to bottom.
pub fn wash_stops(strength: f64) -> [(f64, f64); 3] {
    [(strength, 0.0), (strength * 0.45, 0.42), (strength * 0.14, 1.0)]
}

/// The wash strength of a group: `WASH_SELECTED` on the active tab.
pub fn wash_strength(selected: bool) -> f64 {
    if selected { WASH_SELECTED } else { WASH_OTHER }
}

/// Fill alpha of a tile in the selected group.
pub fn sel_tile_alpha(focused: bool, hovered: bool) -> f64 {
    let hover = match (focused, hovered) {
        (_, false) => 0.0,
        (true, true) => SEL_TILE_HOVER_LIFT - 0.01,
        (false, true) => SEL_TILE_HOVER_LIFT,
    };
    SEL_TILE_ALPHA + if focused { SEL_TILE_FOCUS_LIFT } else { 0.0 } + hover
}

/// The tint of a tab: its Kova colour, or the neutral grey without one.
pub fn tab_tint(color: Option<usize>) -> [f32; 3] {
    color
        .and_then(|c| crate::renderer::TAB_COLORS.get(c).copied())
        .unwrap_or(tokens::TAB_NONE)
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
            TileState::Unread { bell: true } => "bell",
            TileState::Unread { bell: false } => "done",
            TileState::Working => "working",
            TileState::Starting => "starting",
            TileState::Idle => "idle",
            TileState::Shell => "shell",
        }
    }

    /// Colour of the state glyph and the chip text. Neutral states take
    /// `border.strong` for the glyph ring.
    pub fn color(self) -> [f32; 3] {
        match self {
            TileState::Awaiting => tokens::AWAITING,
            TileState::Unread { .. } => tokens::ACCENT,
            TileState::Working | TileState::Starting => tokens::WORKING,
            TileState::Idle | TileState::Shell => tokens::BORDER_STRONG,
        }
    }

    /// The chip's colours: a coloured state keeps its colour at 85 % on
    /// 10 % of the same colour; a neutral chip is tertiary on white 5 %, or
    /// white 55 % on white 7 % inside the selected group.
    pub fn chip_style(self, selected: bool) -> ChipStyle {
        if !self.neutral() {
            let c = self.color();
            ChipStyle { bg: c, bg_alpha: 0.10, fg: c, fg_alpha: 0.85 }
        } else if selected {
            ChipStyle { bg: tokens::WHITE, bg_alpha: 0.07, fg: tokens::WHITE, fg_alpha: 0.55 }
        } else {
            ChipStyle { bg: tokens::WHITE, bg_alpha: 0.05, fg: tokens::TEXT_TERTIARY, fg_alpha: 1.0 }
        }
    }

    /// Neutral chips (idle, shell) are grey, and their glyph is a hollow ring.
    pub fn neutral(self) -> bool {
        matches!(self, TileState::Idle | TileState::Shell)
    }
}

/// A chip's fill and caption, each with its alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChipStyle {
    pub bg: [f32; 3],
    pub bg_alpha: f64,
    pub fg: [f32; 3],
    pub fg_alpha: f64,
}

/// What a collapsed header says about its panes: dots for the awaiting and
/// working panes, then the pane count (`collapsedSummary()` on the phone).
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
    /// `▶`: the hover glyph, and the `▶ Resume` of a shell holding an
    /// agent's resume line.
    Resume,
    /// The awaiting tile's `Open` button.
    Open,
}

impl TileButton {
    /// The Feather icon of the hover button and of the link.
    pub fn icon(self) -> Icon {
        match self {
            TileButton::Close => Icon::X,
            TileButton::Minimize => Icon::Minimize,
            TileButton::Restore => Icon::Maximize,
            TileButton::Stop => Icon::Square,
            TileButton::StartClaude | TileButton::Resume | TileButton::Open => Icon::Play,
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            TileButton::Close => "Close",
            TileButton::Minimize => "Minimize",
            TileButton::Restore => "Restore",
            TileButton::Stop => "Stop",
            TileButton::StartClaude => "Start Claude here",
            TileButton::Resume => "Resume the session",
            TileButton::Open => "Open",
        }
    }

    /// The hover glyphs of a tile, right to left: close, minimize / restore,
    /// stop when something can be interrupted, start Claude on a bare shell,
    /// resume on a shell holding a resume line.
    pub fn hover_glyphs(state: TileState, minimized: bool, bare_shell: bool, resumable: bool) -> Vec<TileButton> {
        let mut out = vec![TileButton::Close, if minimized { TileButton::Restore } else { TileButton::Minimize }];
        if matches!(state, TileState::Working | TileState::Awaiting) {
            out.push(TileButton::Stop);
        }
        if bare_shell {
            out.push(TileButton::StartClaude);
        } else if resumable {
            out.push(TileButton::Resume);
        }
        out
    }
}

/// Copy of the awaiting tile's action line and the shell tile's call. The
/// links carry a filled Feather icon before the word (square for Stop, play
/// for the others).
pub const OPEN_LABEL: &str = "Open";
pub const STOP_LABEL: &str = "Stop";
pub const START_CLAUDE_LABEL: &str = "Start Claude";
pub const RESUME_LABEL: &str = "Resume";

// ---------------------------------------------------------------
// Pane drag arithmetic
// ---------------------------------------------------------------

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

/// `2 waiting · 3 working · 4 idle`, each count its own run so the view
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
            NextPill::Next(_) => "Next unread",
            NextPill::Idle(_) => "Next idle",
            NextPill::CaughtUp => "\u{2713} All caught up",
            NextPill::Nothing => "\u{2713} Nothing to read",
        }
    }

    /// The filled play before the label of the clickable states.
    pub fn icon(self) -> Option<Icon> {
        self.clickable().then_some(Icon::Play)
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(NextPill::Next(1).label(), "Next unread");
        assert_eq!(NextPill::Next(1).icon(), Some(Icon::Play));
        assert_eq!(NextPill::Nothing.icon(), None);
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
        assert_eq!(Unread { bell: true }.chip(), "bell");
        assert_eq!(Working.color(), tokens::WORKING);
        assert_eq!(Shell.color(), tokens::BORDER_STRONG);
    }

    #[test]
    fn chips_keep_their_colour_and_neutral_ones_follow_the_group() {
        use TileState::*;
        let working = Working.chip_style(false);
        assert_eq!(working, ChipStyle { bg: tokens::WORKING, bg_alpha: 0.10, fg: tokens::WORKING, fg_alpha: 0.85 });
        assert_eq!(Working.chip_style(true), working);
        assert_eq!(Awaiting.chip_style(true).fg, tokens::AWAITING);
        assert_eq!(Unread { bell: false }.chip_style(false).fg, tokens::ACCENT);
        let plain = Shell.chip_style(false);
        assert_eq!(plain, ChipStyle { bg: tokens::WHITE, bg_alpha: 0.05, fg: tokens::TEXT_TERTIARY, fg_alpha: 1.0 });
        assert_eq!(Idle.chip_style(false), plain);
        assert_eq!(Idle.chip_style(true), ChipStyle { bg: tokens::WHITE, bg_alpha: 0.07, fg: tokens::WHITE, fg_alpha: 0.55 });
    }

    #[test]
    fn the_wash_fades_from_the_top_and_tiles_lift_under_focus_and_hover() {
        let stops = wash_stops(wash_strength(true));
        assert_eq!(stops[0], (0.40, 0.0));
        assert!((stops[1].0 - 0.18).abs() < 1e-9 && stops[1].1 == 0.42);
        assert!((stops[2].0 - 0.056).abs() < 1e-9 && stops[2].1 == 1.0);
        assert!(stops.windows(2).all(|w| w[0].0 > w[1].0 && w[0].1 < w[1].1));
        let other = wash_stops(wash_strength(false));
        assert_eq!(other[0], (0.20, 0.0));
        assert!((other[1].0 - 0.09).abs() < 1e-9 && other[1].1 == 0.42);
        assert!((other[2].0 - 0.028).abs() < 1e-9 && other[2].1 == 1.0);
        assert_eq!(sel_tile_alpha(false, false), 0.12);
        assert_eq!(sel_tile_alpha(false, true), 0.16);
        assert_eq!(sel_tile_alpha(true, false), 0.22);
        assert!((sel_tile_alpha(true, true) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn buttons_and_links_carry_feather_icons() {
        assert_eq!(TileButton::Close.icon(), Icon::X);
        assert_eq!(TileButton::Minimize.icon(), Icon::Minimize);
        assert_eq!(TileButton::Restore.icon(), Icon::Maximize);
        assert_eq!(TileButton::Stop.icon(), Icon::Square);
        assert_eq!(TileButton::StartClaude.icon(), Icon::Play);
        assert_eq!(TileButton::Resume.icon(), Icon::Play);
        assert_eq!(STOP_LABEL, "Stop");
        assert_eq!(RESUME_LABEL, "Resume");
    }

    #[test]
    fn hover_glyphs_follow_the_tile() {
        use TileButton::*;
        assert_eq!(TileButton::hover_glyphs(TileState::Working, false, false, false), vec![Close, Minimize, Stop]);
        assert_eq!(TileButton::hover_glyphs(TileState::Awaiting, true, false, false), vec![Close, Restore, Stop]);
        assert_eq!(TileButton::hover_glyphs(TileState::Shell, false, true, false), vec![Close, Minimize, StartClaude]);
        assert_eq!(TileButton::hover_glyphs(TileState::Shell, false, false, true), vec![Close, Minimize, Resume]);
        assert_eq!(TileButton::hover_glyphs(TileState::Idle, false, false, false), vec![Close, Minimize]);
    }

    #[test]
    fn a_collapsed_group_counts_waiting_and_working_panes() {
        use TileState::*;
        let s = CollapsedSummary::of([Working, Awaiting, Shell, Working].into_iter());
        assert_eq!(s, CollapsedSummary { awaiting: 1, working: 2, count: 4 });
        assert_eq!(s.count_label(), "4 panes");
        let one = CollapsedSummary::of([Idle].into_iter());
        assert_eq!(one.count_label(), "1 pane");
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
    fn tab_tint_falls_back_to_grey() {
        assert_eq!(tab_tint(None), tokens::TAB_NONE);
        assert_eq!(tab_tint(Some(99)), tokens::TAB_NONE);
        assert_eq!(tab_tint(Some(0)), crate::renderer::TAB_COLORS[0]);
    }
}
