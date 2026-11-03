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
use crate::pane::PaneId;

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
// Colour tokens (the design system, light and dark sets)
// ---------------------------------------------------------------

/// The palette the sidebar is painted with: the design system, generated
/// into `src/window/ds_tokens.rs` from KovaLink's
/// `packages/protocol/tokens/ds.json`. Both sets are generated; the one in
/// use follows the terminal background (see `set_background`), and every call
/// site reads it through `get()`. RGB floats.
pub mod tokens {
    use std::cell::Cell;

    use crate::window::ds_tokens::{dark, light};

    /// One set of the design system's colours, RGB.
    #[allow(dead_code)]
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Palette {
        pub bg2: [f32; 3],
        pub ground: [f32; 3],
        pub tile: [f32; 3],
        pub tile_hover: [f32; 3],
        pub tile_pressed: [f32; 3],
        pub sel_bg: [f32; 3],
        pub sel_text: [f32; 3],
        pub sel_rail: [f32; 3],
        pub border_subtle: [f32; 3],
        pub border_strong: [f32; 3],
        pub accent: [f32; 3],
        pub accent_pressed: [f32; 3],
        pub text_primary: [f32; 3],
        pub text_secondary: [f32; 3],
        pub text_tertiary: [f32; 3],
        pub text_on_fill: [f32; 3],
        pub awaiting: [f32; 3],
        pub working: [f32; 3],
        pub error: [f32; 3],
        pub interrupt: [f32; 3],
        pub tab_none: [f32; 3],
        pub separator: [f32; 3],
        pub ok_dot: [f32; 3],
        pub warn_text: [f32; 3],
        pub info_text: [f32; 3],
        pub crit_text: [f32; 3],
        pub crit_solid: [f32; 3],
        pub crit_solid_hover: [f32; 3],
        pub white: [f32; 3],
        pub black: [f32; 3],
    }

    /// The generated tokens carry alpha; the sidebar paints in RGB.
    const fn rgb(c: [f32; 4]) -> [f32; 3] {
        [c[0], c[1], c[2]]
    }

    /// The solid danger fill, `#ED3726` in both sets: the confirmation's
    /// `Delete`, the one that really deletes. No generated token carries it
    /// in the dark set (`crit.dot` is `#FF6551` there), so it is spelled out
    /// rather than read from either table.
    const CRIT_SOLID: [f32; 3] = [0.929, 0.216, 0.149];
    /// `#CC2314`: the same fill under the mouse and under the press.
    const CRIT_SOLID_HOVER: [f32; 3] = [0.800, 0.137, 0.078];
    /// The white every translucent layer of the selected group is cut from.
    const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
    const BLACK: [f32; 3] = [0.0, 0.0, 0.0];

    pub const DARK: Palette = Palette {
        bg2: rgb(dark::BG2),
        ground: rgb(dark::BG0),
        tile: rgb(dark::BG1),
        tile_hover: rgb(dark::HOVER),
        tile_pressed: rgb(dark::BTN2_HOVER),
        sel_bg: rgb(dark::SEL_BG),
        sel_text: rgb(dark::SEL_TEXT),
        sel_rail: rgb(dark::SEL_RAIL),
        border_subtle: rgb(dark::B3),
        border_strong: rgb(dark::B2),
        accent: rgb(dark::ACCENT),
        accent_pressed: rgb(dark::ACCENT_HOVER),
        text_primary: rgb(dark::T1),
        text_secondary: rgb(dark::T2),
        text_tertiary: rgb(dark::T3),
        text_on_fill: WHITE,
        awaiting: rgb(dark::WARN_DOT),
        working: rgb(dark::INFO_DOT),
        error: rgb(dark::CRIT_DOT),
        interrupt: rgb(dark::CRIT),
        tab_none: rgb(dark::T3),
        separator: rgb(dark::B3),
        ok_dot: rgb(dark::OK_DOT),
        warn_text: rgb(dark::WARN),
        info_text: rgb(dark::INFO),
        crit_text: rgb(dark::CRIT),
        crit_solid: CRIT_SOLID,
        crit_solid_hover: CRIT_SOLID_HOVER,
        white: WHITE,
        black: BLACK,
    };

    pub const LIGHT: Palette = Palette {
        bg2: rgb(light::BG2),
        ground: rgb(light::BG0),
        tile: rgb(light::BG1),
        tile_hover: rgb(light::HOVER),
        tile_pressed: rgb(light::BTN2_HOVER),
        sel_bg: rgb(light::SEL_BG),
        sel_text: rgb(light::SEL_TEXT),
        sel_rail: rgb(light::SEL_RAIL),
        border_subtle: rgb(light::B3),
        border_strong: rgb(light::B2),
        accent: rgb(light::ACCENT),
        accent_pressed: rgb(light::ACCENT_HOVER),
        text_primary: rgb(light::T1),
        text_secondary: rgb(light::T2),
        text_tertiary: rgb(light::T3),
        text_on_fill: WHITE,
        awaiting: rgb(light::WARN_DOT),
        working: rgb(light::INFO_DOT),
        error: rgb(light::CRIT_DOT),
        interrupt: rgb(light::CRIT),
        tab_none: rgb(light::T3),
        separator: rgb(light::B3),
        ok_dot: rgb(light::OK_DOT),
        warn_text: rgb(light::WARN),
        info_text: rgb(light::INFO),
        crit_text: rgb(light::CRIT),
        crit_solid: CRIT_SOLID,
        crit_solid_hover: CRIT_SOLID_HOVER,
        white: WHITE,
        black: BLACK,
    };

    // Which set is up. Thread-local rather than process-wide: every view that
    // paints with these tokens is an AppKit view (`MainThreadOnly`), so the
    // palette only ever changes on the thread that reads it, and a test that
    // switches it cannot be seen by a test running beside it.
    thread_local! {
        static LIGHT_MODE: Cell<bool> = const { Cell::new(false) };
    }

    /// The set the sidebar paints with right now. A pointer pick: a draw call
    /// pays one flag read, not a copy of the palette.
    pub fn get() -> &'static Palette {
        if LIGHT_MODE.with(Cell::get) { &LIGHT } else { &DARK }
    }

    /// Pick the set from the terminal background (`[colors] background`),
    /// which is what the sidebar sits on. Returns whether it changed.
    pub fn set_background(bg: [f32; 3]) -> bool {
        let light = background_is_light(bg);
        LIGHT_MODE.with(|mode| light != mode.replace(light))
    }

    /// Whether a background calls for the light set: the Rec. 709 luma of its
    /// sRGB channels, strictly above 0.5 (`#808080` sits on the line and
    /// stays dark).
    pub fn background_is_light(bg: [f32; 3]) -> bool {
        let [r, g, b] = bg.map(f64::from);
        0.2126 * r + 0.7152 * g + 0.0722 * b > 0.5
    }
}

/// The tint of a tab: its Kova colour, or the neutral grey without one.
pub fn tab_tint(color: Option<usize>) -> [f32; 3] {
    color
        .and_then(|c| crate::renderer::TAB_COLORS.get(c).copied())
        .unwrap_or(tokens::get().tab_none)
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
            SidebarSort::Kova => "Tab order",
            SidebarSort::Activity => "Activity",
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
    /// The last turn ended on a provider error: a limit, an overload, an
    /// expired login. What the transcript says; a running pane wins.
    Error,
    /// Something new nobody looked at: a finished turn, a completion, a bell,
    /// or the user's own mark.
    Unread(UnreadKind),
    Working,
    /// Claude launched, its session not resolved yet.
    Starting,
    /// An agent session sitting open and quiet.
    Idle,
    /// A plain shell.
    Shell,
}

/// Why a tile is unread: what its chip says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnreadKind {
    /// A finished turn, a completion, the hook's waiting flag: `done`.
    Done,
    /// A bell alone: `bell`.
    Bell,
    /// The user's own mark (Cmd+U): `unread`.
    Manual,
}

/// The `Pane` reads a tile state is derived from. `seen` is true for the
/// focused pane of the active tab: what is being looked at is never unread,
/// except by the user's own mark.
#[derive(Clone, Copy, Debug, Default)]
pub struct PaneFlags {
    pub permission_prompt: bool,
    /// The permission prompt appeared since the pane was last looked at.
    pub prompt_unseen: bool,
    pub bell: bool,
    pub completion: bool,
    /// The hook's waiting flag, unseen. Paints `done`, not `waiting`: the
    /// `Stop` hook raises it at every turn end.
    pub hook_unseen: bool,
    pub turn_end_unseen: bool,
    /// Marked unread by hand; survives focus until the pane is focused anew.
    pub manual: bool,
    pub working: bool,
    pub starting: bool,
    pub idle_agent: bool,
    /// The last turn died on an API error (`Pane::has_api_error`).
    pub error: bool,
    pub seen: bool,
}

impl PaneFlags {
    /// The one unread rule (Cmd+J, the pill's count, the tile): something
    /// new since the pane was last looked at, or the user's own mark. Idle
    /// sessions are never unread by themselves.
    pub fn is_unread(self) -> bool {
        self.manual
            || (!self.seen
                && (self.prompt_unseen || self.completion || self.bell || self.hook_unseen || self.turn_end_unseen))
    }
}

impl TileState {
    pub fn from_flags(f: PaneFlags) -> Self {
        if f.permission_prompt && !f.working {
            TileState::Awaiting
        } else if f.error && !f.working {
            TileState::Error
        } else if f.manual {
            TileState::Unread(UnreadKind::Manual)
        } else if !f.seen && (f.completion || f.bell || f.hook_unseen || f.turn_end_unseen) {
            TileState::Unread(if f.bell && !f.completion { UnreadKind::Bell } else { UnreadKind::Done })
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
            TileState::Error => 1,
            TileState::Unread(_) => 2,
            TileState::Working => 3,
            TileState::Starting => 4,
            TileState::Idle => 5,
            TileState::Shell => 6,
        }
    }

    /// The most urgent of a group's states, `Shell` for an empty group.
    pub fn most_urgent(states: impl Iterator<Item = TileState>) -> TileState {
        states.min_by_key(|s| s.rank()).unwrap_or(TileState::Shell)
    }

    /// The state word: what the tile's meta column and the folded header say.
    pub fn chip(self) -> &'static str {
        match self {
            TileState::Awaiting => "waiting",
            TileState::Error => "error",
            TileState::Unread(UnreadKind::Bell) => "bell",
            TileState::Unread(UnreadKind::Done) => "done",
            TileState::Unread(UnreadKind::Manual) => "unread",
            TileState::Working => "working",
            TileState::Starting => "starting",
            TileState::Idle => "idle",
            TileState::Shell => "shell",
        }
    }

    /// Colour of the state dot. Neutral states take `border.strong` for the
    /// glyph ring.
    pub fn color(self) -> [f32; 3] {
        match self {
            TileState::Awaiting => tokens::get().awaiting,
            TileState::Error => tokens::get().error,
            TileState::Unread(_) => tokens::get().accent,
            TileState::Working | TileState::Starting => tokens::get().working,
            TileState::Idle | TileState::Shell => tokens::get().border_strong,
        }
    }

    /// Idle and shell tiles: grey, and their glyph is a hollow ring.
    pub fn neutral(self) -> bool {
        matches!(self, TileState::Idle | TileState::Shell)
    }
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
    /// Interrupt (`■`): the hover glyph of a working or awaiting tile.
    Stop,
    /// The bare shell tile's `▶ Start Claude` link.
    StartClaude,
    /// The `▶ Resume` link of a shell holding an agent's resume line.
    Resume,
    /// `mail`: mark the pane unread by hand.
    MarkUnread,
    /// `check`: mark everything here read.
    MarkRead,
}

impl TileButton {
    /// The Feather icon of the hover button and of the link.
    pub fn icon(self) -> Icon {
        match self {
            TileButton::Close => Icon::X,
            TileButton::Minimize => Icon::Minimize,
            TileButton::Restore => Icon::Maximize,
            TileButton::Stop => Icon::Square,
            TileButton::StartClaude | TileButton::Resume => Icon::Play,
            TileButton::MarkUnread => Icon::Mail,
            TileButton::MarkRead => Icon::Check,
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
            TileButton::MarkUnread => "Mark unread",
            TileButton::MarkRead => "Mark read",
        }
    }

    /// The hover glyphs of a tile, right to left: close, minimize / restore,
    /// mark read / unread, and stop when something can be interrupted. No
    /// play: `Start Claude` and `Resume` are links on line 2, and a click
    /// on the tile already focuses the pane.
    pub fn hover_glyphs(state: TileState, minimized: bool, unread: bool) -> Vec<TileButton> {
        let mut out = vec![
            TileButton::Close,
            if minimized { TileButton::Restore } else { TileButton::Minimize },
            if unread { TileButton::MarkRead } else { TileButton::MarkUnread },
        ];
        if matches!(state, TileState::Working | TileState::Awaiting) {
            out.push(TileButton::Stop);
        }
        out
    }
}

/// Copy of the shell tile's call: a filled play, then the word.
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


// ---------------------------------------------------------------
// Drag geometry (KovaLink `dragSlots.ts`)
// ---------------------------------------------------------------
//
// Where the held tile lands and how far the others step aside. Rows are
// not all the same height; only the gap between them is.

/// A row's position and height in the list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    pub y: f64,
    pub h: f64,
}

/// The rank the held row `from` aims at after moving by `dy`, starting from
/// the `current` rank. A row is crossed when the leading edge of the held
/// tile passes its middle; it is crossed back at its displaced middle, so
/// with a `gap` of hysteresis. The rank stays inside `range` (inclusive).
/// At `dy = 0` nothing moves.
pub fn next_slot(slots: &[Slot], from: usize, current: usize, dy: f64, gap: f64, range: (usize, usize)) -> usize {
    let Some(held) = slots.get(from) else { return current };
    let (lo, hi) = range;
    let big = held.h + gap;
    let top = held.y + dy;
    let bottom = top + held.h;
    let mut to = current.clamp(lo, hi);
    loop {
        let mut next = to;
        if to >= from {
            if to + 1 <= hi && slots.get(to + 1).is_some_and(|below| bottom > below.y + below.h / 2.0) {
                next = to + 1;
            } else if to > from {
                let crossed = slots[to];
                if top < crossed.y - big + crossed.h / 2.0 {
                    next = to - 1;
                }
            }
        }
        if next == to && to <= from {
            if to >= lo + 1 && slots.get(to - 1).is_some_and(|above| top < above.y + above.h / 2.0) {
                next = to - 1;
            } else if to < from {
                let crossed = slots[to];
                if bottom > crossed.y + big + crossed.h / 2.0 {
                    next = to + 1;
                }
            }
        }
        if next == to {
            return to;
        }
        to = next;
    }
}

/// How far each row steps aside when row `from` aims at rank `to`: the
/// room the held tile frees.
pub fn displacements(slots: &[Slot], from: usize, to: usize, gap: f64) -> Vec<f64> {
    let big = slots.get(from).map_or(0.0, |s| s.h) + gap;
    (0..slots.len())
        .map(|i| {
            if from < i && i <= to {
                -big
            } else if to <= i && i < from {
                big
            } else {
                0.0
            }
        })
        .collect()
}

/// Where the held tile lands on release, relative to its origin: what the
/// skeleton's position is checked against.
#[cfg(test)]
pub fn settle_offset(slots: &[Slot], from: usize, to: usize) -> f64 {
    let (Some(held), Some(target)) = (slots.get(from), slots.get(to)) else { return 0.0 };
    if to == from {
        0.0
    } else if to > from {
        target.y + target.h - held.h - held.y
    } else {
        target.y - held.y
    }
}

/// The top of the skeleton marking the aimed rank: the room left by the
/// rows that stepped aside, which is exactly where the tile will land.
pub fn placeholder_y(slots: &[Slot], from: usize, to: usize, gap: f64) -> f64 {
    let (Some(held), Some(target)) = (slots.get(from), slots.get(to)) else {
        return slots.get(from).map_or(0.0, |s| s.y);
    };
    if to == from {
        held.y
    } else if to > from {
        target.y + target.h - (held.h + gap) + gap
    } else {
        target.y
    }
}

/// The travel the held tile may take: from the top of the first row of the
/// range to the bottom of the last, as `(min_dy, max_dy)`.
pub fn drag_bounds(slots: &[Slot], from: usize, range: (usize, usize)) -> (f64, f64) {
    let (Some(held), Some(first), Some(last)) = (slots.get(from), slots.get(range.0), slots.get(range.1)) else {
        return (0.0, 0.0);
    };
    (first.y - held.y, last.y + last.h - held.h - held.y)
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

/// What the Next pill shows (KovaLink `NextPill.tsx`): the unread count, or
/// nothing to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextPill {
    /// Unread panes left: accent pill, count badge, jumps to the next one.
    Next(usize),
    /// Nothing to read: raised ground, tertiary text, not a button.
    Nothing,
}

impl NextPill {
    pub fn of(unread: usize) -> Self {
        if unread > 0 { NextPill::Next(unread) } else { NextPill::Nothing }
    }

    pub fn label(self) -> &'static str {
        match self {
            NextPill::Next(_) => "Next",
            NextPill::Nothing => "Nothing to read",
        }
    }

    /// The filled play before the label of the clickable states.
    pub fn icon(self) -> Option<Icon> {
        self.clickable().then_some(Icon::Play)
    }

    pub fn badge(self) -> Option<usize> {
        match self {
            NextPill::Next(n) => Some(n),
            NextPill::Nothing => None,
        }
    }

    pub fn clickable(self) -> bool {
        matches!(self, NextPill::Next(_))
    }
}

/// How many panes of `order` are unread: the pill's count.
pub fn unread_count(order: &[(PaneId, bool)]) -> usize {
    order.iter().filter(|&&(_, unread)| unread).count()
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
    }

    /// Rows stacked from y 0 with an 8 pt gap.
    fn stack(heights: &[f64]) -> Vec<Slot> {
        let mut y = 0.0;
        heights
            .iter()
            .map(|&h| {
                let slot = Slot { y, h };
                y += h + 8.0;
                slot
            })
            .collect()
    }

    #[test]
    fn a_held_tile_crosses_rows_at_their_middle_with_hysteresis() {
        let equal = stack(&[64.0, 64.0, 64.0, 64.0]);
        let all = (0, 3);
        for i in 0..4 {
            assert_eq!(next_slot(&equal, i, i, 0.0, 8.0, all), i);
        }
        // Down: the next row's middle is at 104, the held bottom at 64 + dy.
        assert_eq!(next_slot(&equal, 0, 0, 40.0, 8.0, all), 0);
        assert_eq!(next_slot(&equal, 0, 0, 41.0, 8.0, all), 1);
        assert_eq!(next_slot(&equal, 0, 0, 120.0, 8.0, all), 2);
        // Up: the previous row's middle is at 104, the held top at 144 + dy.
        assert_eq!(next_slot(&equal, 2, 2, -40.0, 8.0, all), 2);
        assert_eq!(next_slot(&equal, 2, 2, -41.0, 8.0, all), 1);
        assert_eq!(next_slot(&equal, 3, 3, -300.0, 8.0, all), 0);
        // Hysteresis: a crossed row is crossed back at its displaced middle.
        assert_eq!(next_slot(&equal, 0, 1, 41.0, 8.0, all), 1);
        assert_eq!(next_slot(&equal, 0, 1, 33.0, 8.0, all), 1);
        assert_eq!(next_slot(&equal, 0, 1, 31.0, 8.0, all), 0);
        assert_eq!(next_slot(&equal, 2, 1, -41.0, 8.0, all), 1);
        assert_eq!(next_slot(&equal, 2, 1, -33.0, 8.0, all), 1);
        assert_eq!(next_slot(&equal, 2, 1, -31.0, 8.0, all), 2);
        // Unequal heights: the crossed row's middle counts, not its height.
        let mixed = stack(&[64.0, 220.0, 64.0]);
        assert_eq!(next_slot(&mixed, 0, 0, 118.0, 8.0, (0, 2)), 0);
        assert_eq!(next_slot(&mixed, 0, 0, 119.0, 8.0, (0, 2)), 1);
        assert_eq!(next_slot(&mixed, 1, 1, -40.0, 8.0, (0, 2)), 1);
        assert_eq!(next_slot(&mixed, 1, 1, -41.0, 8.0, (0, 2)), 0);
        // Bounded by the list and by the range.
        assert_eq!(next_slot(&equal, 0, 0, -500.0, 8.0, all), 0);
        assert_eq!(next_slot(&equal, 3, 3, 500.0, 8.0, all), 3);
        assert_eq!(next_slot(&equal, 0, 0, 500.0, 8.0, all), 3);
        assert_eq!(next_slot(&equal, 2, 2, -500.0, 8.0, (2, 3)), 2);
        assert_eq!(next_slot(&equal, 2, 2, 500.0, 8.0, (2, 3)), 3);
        assert_eq!(next_slot(&equal, 1, 1, 500.0, 8.0, (0, 1)), 1);
        assert_eq!(next_slot(&equal, 9, 1, 50.0, 8.0, all), 1);
    }

    #[test]
    fn rows_step_aside_by_the_held_height_and_the_skeleton_takes_the_landing_spot() {
        let equal = stack(&[64.0, 64.0, 64.0, 64.0]);
        assert_eq!(displacements(&equal, 0, 2, 8.0), vec![0.0, -72.0, -72.0, 0.0]);
        assert_eq!(displacements(&equal, 3, 1, 8.0), vec![0.0, 72.0, 72.0, 0.0]);
        assert_eq!(displacements(&equal, 1, 1, 8.0), vec![0.0; 4]);
        let mixed = stack(&[64.0, 220.0, 64.0]);
        assert_eq!(displacements(&mixed, 1, 2, 8.0), vec![0.0, 0.0, -228.0]);
        assert_eq!(settle_offset(&equal, 0, 0), 0.0);
        assert_eq!(settle_offset(&equal, 0, 2), 144.0);
        assert_eq!(settle_offset(&equal, 2, 0), -144.0);
        assert_eq!(settle_offset(&mixed, 0, 1), 292.0 - 64.0);
        assert_eq!(settle_offset(&mixed, 1, 0), -72.0);
        assert_eq!(drag_bounds(&equal, 1, (0, 3)), (-72.0, 144.0));
        assert_eq!(drag_bounds(&equal, 0, (0, 3)), (0.0, 216.0));
        assert_eq!(drag_bounds(&equal, 2, (2, 3)), (0.0, 72.0));
        assert_eq!(drag_bounds(&mixed, 0, (0, 2)), (0.0, 300.0));
        assert_eq!(drag_bounds(&equal, 7, (0, 3)), (0.0, 0.0));
        assert_eq!(placeholder_y(&equal, 0, 0, 8.0), 0.0);
        assert_eq!(placeholder_y(&equal, 2, 2, 8.0), 144.0);
        // The skeleton sits exactly where the tile will land.
        for slots in [equal, stack(&[72.0, 88.0, 210.0]), stack(&[210.0, 72.0, 88.0, 72.0])] {
            for from in 0..slots.len() {
                for to in 0..slots.len() {
                    assert_eq!(placeholder_y(&slots, from, to, 8.0), slots[from].y + settle_offset(&slots, from, to));
                }
            }
        }
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
    fn the_next_pill_counts_the_unread_panes_or_says_nothing() {
        assert_eq!(NextPill::of(3), NextPill::Next(3));
        assert_eq!(NextPill::of(1), NextPill::Next(1));
        assert_eq!(NextPill::of(0), NextPill::Nothing);
        assert_eq!(NextPill::Next(3).badge(), Some(3));
        assert_eq!(NextPill::Nothing.badge(), None);
        assert!(NextPill::Next(1).clickable());
        assert!(!NextPill::Nothing.clickable());
        assert_eq!(NextPill::Next(1).label(), "Next");
        assert_eq!(NextPill::Nothing.label(), "Nothing to read");
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
            prompt_unseen: true,
            bell: true,
            completion: true,
            hook_unseen: true,
            turn_end_unseen: true,
            manual: false,
            working: true,
            starting: true,
            idle_agent: true,
            error: true,
            seen: false,
        };
        // A prompt on screen loses to the spinner: Claude went back to work.
        assert_eq!(TileState::from_flags(all), Unread(UnreadKind::Done));
        assert_eq!(TileState::from_flags(PaneFlags { working: false, ..all }), Awaiting);
        assert_eq!(TileState::from_flags(PaneFlags { permission_prompt: false, completion: false, ..all }), Unread(UnreadKind::Bell));
        // The hook flag alone paints done, never waiting.
        assert_eq!(TileState::from_flags(PaneFlags { hook_unseen: true, ..PaneFlags::default() }), Unread(UnreadKind::Done));
        assert_eq!(TileState::from_flags(PaneFlags { turn_end_unseen: true, ..PaneFlags::default() }), Unread(UnreadKind::Done));
        // An API error shows as such, but never over a turn in flight.
        let failed = PaneFlags { error: true, idle_agent: true, ..PaneFlags::default() };
        assert_eq!(TileState::from_flags(failed), Error);
        assert_eq!(TileState::from_flags(PaneFlags { working: true, ..failed }), Working);
        // It outranks unread output: the error is what the turn ended on.
        assert_eq!(TileState::from_flags(PaneFlags { turn_end_unseen: true, ..failed }), Error);
        assert_eq!(Error.chip(), "error");
        assert_eq!(Error.color(), tokens::get().error);
        assert!(Error.rank() < Unread(UnreadKind::Done).rank());
        // The pane being looked at never shows unread, unless marked by hand.
        assert_eq!(TileState::from_flags(PaneFlags { permission_prompt: false, seen: true, ..all }), Working);
        assert_eq!(TileState::from_flags(PaneFlags { permission_prompt: false, seen: true, manual: true, ..all }), Unread(UnreadKind::Manual));
        assert_eq!(TileState::from_flags(PaneFlags { manual: true, ..PaneFlags::default() }), Unread(UnreadKind::Manual));
        assert_eq!(TileState::from_flags(PaneFlags { starting: true, idle_agent: true, ..PaneFlags::default() }), Starting);
        assert_eq!(TileState::from_flags(PaneFlags { idle_agent: true, ..PaneFlags::default() }), Idle);
        assert_eq!(TileState::from_flags(PaneFlags::default()), Shell);
        assert!(Idle.neutral() && Shell.neutral() && !Working.neutral());
        assert_eq!(Unread(UnreadKind::Bell).chip(), "bell");
        assert_eq!(Unread(UnreadKind::Manual).chip(), "unread");
        assert_eq!(Working.color(), tokens::get().working);
        assert_eq!(Shell.color(), tokens::get().border_strong);
    }

    #[test]
    fn a_pane_is_unread_when_something_is_new_or_marked_and_never_for_being_idle() {
        let quiet = PaneFlags::default();
        assert!(!quiet.is_unread());
        assert!(!PaneFlags { idle_agent: true, ..quiet }.is_unread());
        assert!(!PaneFlags { working: true, ..quiet }.is_unread());
        // A prompt on screen counts only while unseen.
        assert!(PaneFlags { permission_prompt: true, prompt_unseen: true, ..quiet }.is_unread());
        assert!(!PaneFlags { permission_prompt: true, ..quiet }.is_unread());
        for f in [
            PaneFlags { bell: true, ..quiet },
            PaneFlags { completion: true, ..quiet },
            PaneFlags { hook_unseen: true, ..quiet },
            PaneFlags { turn_end_unseen: true, ..quiet },
        ] {
            assert!(f.is_unread());
            // Under the eye, the same signals are read.
            assert!(!PaneFlags { seen: true, ..f }.is_unread());
        }
        // The manual mark counts even on the pane being looked at.
        assert!(PaneFlags { manual: true, ..quiet }.is_unread());
        assert!(PaneFlags { manual: true, seen: true, ..quiet }.is_unread());
    }

    #[test]
    fn buttons_and_links_carry_feather_icons() {
        assert_eq!(TileButton::Close.icon(), Icon::X);
        assert_eq!(TileButton::Minimize.icon(), Icon::Minimize);
        assert_eq!(TileButton::Restore.icon(), Icon::Maximize);
        assert_eq!(TileButton::Stop.icon(), Icon::Square);
        assert_eq!(TileButton::StartClaude.icon(), Icon::Play);
        assert_eq!(TileButton::Resume.icon(), Icon::Play);
        assert_eq!(RESUME_LABEL, "Resume");
    }

    #[test]
    fn hover_glyphs_follow_the_tile() {
        use TileButton::*;
        assert_eq!(TileButton::hover_glyphs(TileState::Working, false, false), vec![Close, Minimize, MarkUnread, Stop]);
        assert_eq!(TileButton::hover_glyphs(TileState::Awaiting, true, true), vec![Close, Restore, MarkRead, Stop]);
        // A shell, bare or holding a resume line, gets no play glyph: its
        // `Start Claude` / `Resume` link already sits on line 2.
        assert_eq!(TileButton::hover_glyphs(TileState::Shell, false, false), vec![Close, Minimize, MarkUnread]);
        assert_eq!(TileButton::hover_glyphs(TileState::Idle, false, true), vec![Close, Minimize, MarkRead]);
        assert_eq!(MarkUnread.icon(), Icon::Mail);
        assert_eq!(MarkRead.icon(), Icon::Check);
    }

    #[test]
    fn a_collapsed_group_counts_waiting_and_working_panes() {
        use TileState::*;
        let s = CollapsedSummary::of([Working, Awaiting, Shell, Working].into_iter());
        assert_eq!(s, CollapsedSummary { awaiting: 1, working: 2, count: 4 });
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
        assert_eq!(SidebarSort::Activity.label(), "Activity");
    }

    #[test]
    fn the_background_luma_picks_the_palette() {
        // White text on a light ground would be unreadable, so a light ground
        // gets the light set; `#808080` sits exactly on the line and stays
        // dark.
        assert!(tokens::background_is_light([1.0, 1.0, 1.0]));
        assert!(!tokens::background_is_light([0.118, 0.118, 0.118]));
        assert!(!tokens::background_is_light([0.5, 0.5, 0.5]));
        // The two sets are really different tables, not one read twice.
        assert_ne!(tokens::LIGHT.ground, tokens::DARK.ground);
        assert_ne!(tokens::LIGHT.text_primary, tokens::DARK.text_primary);
    }

    #[test]
    fn a_config_reload_switches_the_palette_and_keeps_crit_solid() {
        assert_eq!(tokens::get().ground, tokens::DARK.ground);
        assert!(tokens::set_background([1.0, 1.0, 1.0]));
        assert_eq!(tokens::get().ground, tokens::LIGHT.ground);
        assert_eq!(tokens::get().text_primary, tokens::LIGHT.text_primary);
        // `#ED3726` in both sets: the one that really deletes never moves.
        assert_eq!(tokens::get().crit_solid, [0.929, 0.216, 0.149]);
        // The same palette again changes nothing, and a dark background goes
        // back to the dark set.
        assert!(!tokens::set_background([1.0, 1.0, 1.0]));
        assert!(tokens::set_background([0.1, 0.1, 0.12]));
        assert_eq!(tokens::get().ground, tokens::DARK.ground);
        assert_eq!(tokens::get().text_primary, tokens::DARK.text_primary);
        assert_eq!(tokens::get().crit_solid, [0.929, 0.216, 0.149]);
    }

    #[test]
    fn tab_tint_falls_back_to_grey() {
        assert_eq!(tab_tint(None), tokens::get().tab_none);
        assert_eq!(tab_tint(Some(99)), tokens::get().tab_none);
        assert_eq!(tab_tint(Some(0)), crate::renderer::TAB_COLORS[0]);
    }
}
