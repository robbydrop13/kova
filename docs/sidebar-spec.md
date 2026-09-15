# Sidebar layout mode (v3, AppKit)

A second way to show a window's tabs: instead of the strip across the top, a
sticky column down the left edge listing every tab as a group and every pane
as a tile with its state at a glance, the way the KovaLink home screen does
it. v3 moves the sidebar out of the Metal terminal renderer into a native
AppKit view: SF Pro, rounded tiles and chips, native scrolling, tooltips and
cursor rects, so the Mac reads like the phone. Everything v2 did (tiles,
chips, summary, Next pill, one-click actions, context menus, drag reorder,
prompt preview) is kept; only the drawing and the hit testing moved.

Code:

- `src/window/sidebar.rs`: pure. The process-wide layout setting and its
  persistence, the colour tokens, `TileState` and its priority, the activity
  sort, `CollapsedSummary`, `TileButton`, the Next pill, the pane drag
  arithmetic (`swap_chain`, `drop_index`). Unit tests.
- `src/window/sidebar_model.rs`: pure. `SidebarModel { summary, sort, pill,
  groups: Vec<GroupVm { tiles: Vec<TileVm> }>, show_hint }`, built from
  `TabFacts` / `PaneFacts` (plain reads of the tabs), and the tile identity
  rules (`tile_title`, `project_name`, `subtitle`). `PartialEq`, so the view
  can tell whether a tick changed anything. Unit tests.
- `src/window/sidebar_view.rs`: the AppKit side. `ListLayout` and
  `ChromeLayout` (pure geometry in points, hit tests, drag slots; text widths
  through the `TextMetrics` trait, unit tests with a stand-in), then two
  `NSView` subclasses drawn with `drawRect:`: `SidebarView` (chrome: top
  area, summary row, Next pill, resize edge, and an `NSScrollView`)
  and `SidebarListView` (its document: groups and tiles, hover, press, drags,
  right click, tooltips).
- `src/window/sidebar_ui.rs`: the window's side. `apply_layout` (the split
  between the sidebar and the Metal view), the mode switch, `sync_sidebar`
  (once per tick: tabs -> `SidebarModel` -> view), the actions the view calls
  back and the context menus.
- `src/prompt_preview.rs` and `Pane::probe_prompt`: the permission prompt and
  turn-end summary, unchanged from v2 (section 4.1).

## 1. Principles

1. Readable from across the room: a coloured dot per tile, a tinted chip
   with the state word, a colour bar per tab; the title at 1 m.
2. Same structure and vocabulary as KovaLink: tab groups, tiles, `waiting` /
   `working` / `starting` / `done` / `bell` / `idle` / `shell` chips, the
   awaiting card with its question, the Next pill, the sort toggle, the
   collapsed summary, the `+` on a group. Same colour tokens
   (`sidebar::tokens`, from `link/app/src/theme/tokens.ts`).
3. Same actions as the phone's swipes and sheets, one click away: Open, Stop,
   Close, Rename, Bookmark, Minimize / Restore, Start Claude, Resume, Add a
   pane.
4. Same words as the phone's session rows (`SessionRow.tsx`): the tile is
   titled by the session name, subtitled by the project and the agent; a
   directory is never a title.
5. Native where it is free: SF Pro, `NSScrollView` (elastic, overlay
   scroller), `scrollRectToVisible`, `autoscroll:`, tooltip rects, cursor
   rects, `NSMenu`, `NSAlert`.
6. Zero cost when off: in tabs mode the sidebar view is hidden and
   `sync_sidebar` returns at once; the tab bar and status bar are untouched.

## 2. Window structure

The window's content view is a plain container holding two siblings: the
`SidebarView` at the left and the `KovaView` (Metal) at the right.
`KovaView::apply_layout` splits the container: sidebar `[0, width]`, Metal
view the rest; in tabs mode, or when `container_w - width < splits.min_width`
(the narrow fallback), the sidebar is hidden and the Metal view takes the
whole width. It runs on creation, on every container resize
(`resizeWithOldSuperviewSize:`), on a mode switch and during an edge drag.
Setting the Metal view's frame runs `handle_resize`, which hands every pane
its new size, so the terminal never reserves space for the sidebar itself:
its viewport is its own frame (`drawable_viewport`). `crate::app::kova_view`
finds the `KovaView` one level under the content view.

The sidebar spans the full window height; the global status bar of the Metal
view spans the pane area only. Both views keep `mouseDownCanMoveWindow` off;
the sidebar's top area drags the window itself.

## 3. Layout (points, 1x; AppKit handles Retina)

### 3.1 Width

`layout.sidebar_width` in points, default 280, clamped to 200..520
(`config::SIDEBAR_WIDTH_RANGE`); a value below 100 is a v2 cell count and is
read as 8 pt per cell. The right edge is a 1 pt `border.subtle` separator,
part of the width; a resize handle 4 pt either side (cursor
`resizeLeftRight` through a cursor rect). The drop rule is
`width = round(x)` on every drag event, panes resized live, persisted on
mouse up.

### 3.2 Vertical regions (flipped coordinates, y down)

| Region   | Height | Content                                                         |
|----------|--------|-----------------------------------------------------------------|
| Top area | 36     | Traffic lights, window drag (`performWindowDragWithEvent`), double click = zoom |
| Summary  | 24     | Left: `2 waiting · 3 working · 4 idle`; right: `⇅ kova` / `⇅ activity` |
| Pill     | 36     | The Next pill, 28 tall, radius 14, content-sized, right-aligned 12 from the edge |
| List     | rest   | `NSScrollView` down to the bottom, width = inner width - 4 (the edge zone stays the chrome's) |

No footer: the way back to the tab bar is View > Show Tab Bar and ⌥⌘S
(section 7); the version label stays in the tab bar of tabs mode.

### 3.3 Typography (`sidebar_view::Style`, `NSFont::systemFontOfSize_weight`)

KovaLink's row scale, so the Mac reads like the phone: header and tile
titles 15 semibold (`calloutStrong`); secondary line and question 13
(`footnote`); detail, links and pill label 12 (links and pill semibold);
chips, summary, hint, number 11 (chips medium, `caption`); sort toggle and
pill badge 10 (sort medium, badge bold); `+` 14 medium. Text colours
are the phone's: primary `#E8EAED`, secondary `#9BA3AF`, tertiary `#7C8593`.
Truncation is AppKit's: titles and subtitles at the tail (never a head-cut
`..jects/Acme`), rename edit buffers at the head (so the `▏` cursor stays
visible), the question wrapped to two lines with an ellipsis on the last
(`TruncatesLastVisibleLine`).

### 3.4 Group (one per tab; `TabGroupView.tsx`)

Content inset 12 on each side; groups 16 apart. Every group is a panel
(option "A. Wash"), radius 12, filled `bg.raised` and washed with a vertical
gradient of the tint (`sidebar::wash_stops(strength)`, an `NSGradient` in
the panel's rounded path): `strength` at the top, x 0.45 at 42 % of the
height, x 0.14 at the bottom; `strength` is 0.40 on the selected tab
(`WASH_SELECTED`) and on the tab under the mouse (`wash_strength(selected,
hovered)`: any hit inside the panel, controls and tiles included, bound as
`hovered_tab`, a tab id, on every mouse move), 0.20 on the others
(`WASH_OTHER`). The whole panel is one click target: a click on its
padding, on a gap or on the empty bottom (`ListHit::Panel`, the fallback
once no row or control took the point) selects the tab, and the hand
cursor covers every panel (cursor rects on the list view, refreshed on
relayout). The header sits at
the top of the panel, the tiles 12 pt apart (`PANEL_GAP`), and the panel
pads 12 pt all around (`PANEL_PAD`: above the header, at the sides, under
the last row). No tint bar. The tint is `TAB_COLORS[c]` (twelve colours:
red, orange, yellow, green, blue, violet, pink, coral, lime, teal, cyan,
indigo) or `tabNone` grey.

Header 28 tall, its hover ground a full pill (radius 14), no ground of its
own otherwise (the wash shows through):
the `chevron-down` / `chevron-right` icon, the 8 pt tint dot, the Cmd+N
number, the title (15 semibold, primary), then at the right the `+` button
(24 pt, radius 8) and, when folded, the collapsed summary: a 6 pt amber dot
when a pane awaits, a 6 pt blue dot when one works, `4 panes`. Row hover:
white 5 %; pressed 10 %. The `+` is hidden until the mouse is anywhere on
the header row (dot and `+` included); it has no ground of its own, and
white 14 % (radius 8) appears under it only while the mouse is on the `+`
itself (20 % pressed). The dot is a button (section 4.3, colour picker): its
16 pt box (`ListLayout::dot_button`) is its own hit (`ListHit::HeaderDot`),
between the chevron zone and the number, and a 1 pt white ring at 20 %
(35 % pressed) appears around it under the mouse.

#### 3.4.1 Selected tab

The selected tab's panel differs by its stronger wash and by its text:

- Header: title white; number, chevron and collapsed count white 72 %
  (secondary / tertiary elsewhere); the dot in the tint with a 3 pt halo at
  28 %. The `+` is white when shown (primary elsewhere).
- Tiles (every panel): white 12 % with a 1 pt border white 6 %, radius 10;
  hover +4 %; pressed +6 %; minimized 7 % less. In the selected panel the
  secondary text is white 68 % and the idle / shell ring white 45 %.
  Focused pane (selected panel only): white 22 % (+3 % under the mouse), a
  1 pt border white 22 % and a soft shadow (`NSShadow`, 0 / 6 pt down, 18 pt
  blur, black 22 %); the accent ring is gone, the focused tile is simply the
  brightest one. The awaiting card keeps its amber look inside the panel.
- Chips (`TileState::chip_style(selected)`): neutral chips are white 55 % on
  white 7 % in the selected panel; `text.tertiary` on white 5 % elsewhere.
  Awaiting, working, starting and unread chips keep their colour at 85 % on
  10 % of the same colour, in both places.

### 3.5 Tile (`SessionRow.tsx`), radius 10, fill `bg.raised`, padding 8 v / 10 h

Row 1 (20 tall): the state glyph at x 10 (an 8 pt dot in the state colour;
a 1.5 pt `border.strong` ring for idle and shell), the title from x 26 (`⊟ `
prefix when minimized), the chip right-aligned: 18 tall, radius 9, padding 7,
its colours from `TileState::chip_style` (3.4.1): working 85 % on working
10 % (`working`, `starting`), accent 85 % on accent 10 % + 6 pt accent dot
(`done`, `bell`), tertiary on white 5 % (`idle`, `shell`, or the agent of a
restored session: see below). Row 2 (16 tall, 2 below): the subtitle in
secondary, a `★` in amber first when bookmarked; a bare shell shows
`Start Claude` and a restored session `Resume`, each after a filled 11 pt
`play` (accent link, primary on hover) right-aligned instead of the end of
the subtitle. An unread tile with a turn-end summary adds a third row in
secondary. Heights 54 / 72. Fill, border, hover, pressed, minimized and
the focused pane: 3.4.1 (white layers over the panel's wash, no accent
ring).

Identity (`sidebar_model::tile_title`, `subtitle`; the phone's `paneLabel`
and `SessionRow` subtitle, from the same `Pane` reads the daemon gets over
IPC):

- Title: the agent session name (`/rename`), else the pane's own title
  (`custom_title`, then the OSC title) unless it names a directory (the cwd or
  its basename, anything with a `/`, `~…`, zsh's head-cut `..jects/Acme`,
  a `user@host:~/dir` prompt title: the shell writes those into OSC 1, which
  Kova keeps as the sticky title), else the agent (`claude`, `codex`), else
  the foreground process, else `Shell`. Tail-truncated.
- Subtitle: `project · agent`, `project` being the cwd's last segment (the
  phone's `projectName`), the agent left out when it is already the title, and
  the project alone on a plain shell.
- Agent: the live one (`Pane::agent_kind()`), else the one whose resume line
  waits at the prompt (`Pane::restored_session()`: a bare shell whose last
  command is `claude … --resume <id>` or `codex resume <id>`, which is what a
  restored pane holds until Enter is pressed). Such a restored session keeps
  the shell tile (ring) but its chip reads `claude` / `codex` and its call is
  `Resume`; `shell` and `Start Claude` are for a plain shell only.
- The group header without a custom tab name is titled the same way from the
  focused pane.

Hover actions replace the chip on row 1, right to left: `x` close (error
red on hover), `minimize-2` / `maximize-2` minimize / restore, `check` mark
read (unread tile) or `mail` mark unread (read tile), and `square` stop
(interrupt colour; working or awaiting only). No play glyph: `Start Claude`
and `Resume` are the links on row 2, and a click on the tile already
focuses the pane. Each is a 24 pt button (radius 7, bare; white 12 % under
the one hovered, 20 % pressed) holding a 16 pt Feather icon, 2 apart, with
a tooltip (`Close`, `Minimize`, `Restore`, `Mark read`, `Mark unread`,
`Stop`) through `addToolTipRect:owner:userData:`. Pressed paints on mouse
down, the action fires on mouse up inside the same button (a leave
cancels).

#### Icon set (`src/window/feather.rs`)

Feather icons, drawn as stroked `NSBezierPath`s on Feather's 24 pt grid
(2 pt stroke, round caps and joins) mapped into a 16 pt box centred in the
button, so the stroke is 1.33 pt and the icon scales with the row. The
views are flipped (y down, like SVG), so the published coordinates are used
as they are. `chevron-down` / `chevron-right` (collapse caret), `plus`
(header), `x` (close), `maximize-2` / `minimize-2` (restore / minimize),
`square` (stop), `check` / `mail` (mark read / unread). The `Start Claude`,
`Resume` and `Stop` links and the Next pill use the filled `play` and
`square` (11 pt in the links, 12 pt in the pill). `Icon::segments` and the point mapping are pure
and unit-tested.

### 3.6 Awaiting card (`AwaitingCard.tsx`), radius 12, fill `awaitingBg`

A 4 pt amber bar at the left edge (clipped by the radius), a 1 pt amber
border at 35 % (80 % on hover), padding 10 v / 12 h. Row 1: amber dot, title,
the age (`4m`) right in tertiary, `status.error` past 10 min. Then the
question (13, primary, 1 or 2 lines, measured), the detail (12, secondary:
the command, the file, or the header), and a 20 pt actions row: `Open`
(accent pill, white 12 semibold, `primaryPressed` on hover) at the text
column, `Stop` after a filled `square` (interrupt colour, primary on hover) at the right. Heights
82 (1 line, no detail) to 116.

### 3.7 Summary, sort, Next pill

- Summary runs: `N waiting` amber, `N working` blue (starting counts as
  working), `N idle` tertiary, dots tertiary; zero counts omitted; all zero:
  `nothing running`. Counts cover this window; the pill counts cover every
  window (like Cmd+J).
- Sort toggle: `⇅ kova` tertiary / `⇅ activity` accent, primary on hover on
  a `bg.pressed` rounded ground.
- Next pill (`sidebar::NextPill`, from KovaLink `NextPill.tsx`), one
  unread model (section 5.2):

| State   | Condition          | Fill             | Text                                | Badge                             |
|---------|--------------------|------------------|-------------------------------------|-----------------------------------|
| next    | unread count > 0   | `accent.primary` | white filled `play` + `Next unread` | white 18 pt circle, accent digits |
| nothing | unread count == 0  | `bg.raised`      | tertiary `Nothing to read`          | none; not a button (no hover, no press) |

  The pill is sized to its content (`sidebar_view::pill_width`: 12 pt
  padding either side, the 12 pt play and a 5 pt gap when clickable, the
  label, an 8 pt gap and the badge when there is a count; the badge is 18
  wide at least, `digits + 8` beyond) and right-aligned 12 pt from the
  sidebar's edge on its own row; a sidebar too narrow clamps it to the
  inset width. The `nothing` state is content-sized and right-aligned the
  same way. The hit region is the pill's rect. No `⌘J` inside the pill: the
  shortcut lives in the help overlay and the README. Hover:
  `accent.primaryPressed`. Pressed: text alpha 0.85. The count is
  `KovaView::collect_unread` (`src/window/attention.rs`), the same list
  `do_focus_next_attention` walks, read once per tick. A change of the pill's
  content re-lays the chrome, like a change of the sort label.

### 3.8 List extras

- Hint `⌘T new tab · ⌘D split` (11, tertiary, centred) under the last group
  when the window holds fewer than 3 panes.
- Overflow: the scroll view's overlay scroller and elastic bounce; the
  content height is the document view's frame.
- Reveal: on every focus change the list scrolls the least that shows the
  focused pane's tile (`scrollRectToVisible`, with the tile gap around it);
  if its group is folded, the header.

## 4. Interaction

### 4.1 Mouse

| Target                     | Click                                                  | Double click       | Right click  |
|----------------------------|--------------------------------------------------------|--------------------|--------------|
| Panel padding, gaps, bottom| `do_switch_tab(idx)`                                   |                    | header menu  |
| Header chevron zone (24 pt)| toggle collapse, no tab switch                         | same               | header menu  |
| Header colour dot (16 pt)  | colour picker (4.3), no tab switch                     |                    | header menu  |
| Header body                | `do_switch_tab(idx)`; on the active tab: toggle collapse | `start_rename_tab` | header menu |
| Header `+`                 | add a pane (4.4)                                       |                    | header menu  |
| Tile body                  | `focus_pane_in_window(id)` (switches tab, restores, reveals) | rename pane  | tile menu    |
| Tile hover buttons         | the action, without focusing the pane                  |                    | tile menu    |
| Awaiting `Open`            | same as tile body                                      |                    | tile menu    |
| Awaiting `Stop`            | interrupt (4.4)                                        |                    | tile menu    |
| Shell `Start Claude`       | start Claude (4.4)                                     |                    | tile menu    |
| Restored `Resume`          | resume (4.4)                                           |                    | tile menu    |
| Next pill                  | `do_focus_next_attention()` (not when `nothing`)       |                    |              |
| Sort toggle                | kova <-> activity                                      |                    |              |
| Top area                   | window drag                                            | zoom               |              |
| Separator +-4 pt           | drag resizes the sidebar                               |                    |              |

The wheel over the chrome is forwarded to the scroll view; over the list the
scroll view has it. Neither sidebar view accepts first responder: the
keyboard stays with the terminal.

### 4.2 Drag

- Tab drag: mouse down on a header body, move 3 pt: the header lifts (drawn
  at 35 % in place, a copy at 90 % under the cursor) and a 2 pt accent
  insertion line spans the content width between the two groups the cursor
  is over (midpoint rule over headers). The drop goes through
  `sidebar_reorder_tab` -> `ipc_move_tab`. Disabled in activity sort.
- Pane drag (KovaLink's `dragSlots.ts` / `dragMachine.ts`, ported to
  `sidebar::next_slot`, `displacements`, `placeholder_y`, `drag_bounds`
  and `ListLayout::pane_drag_frame`): mouse down on a tile body, move 3 pt:
  the tile lifts into a ghost (its tile drawn on a `bg.raised` ground with a
  shadow, 0 / 6 pt down, 12 pt blur, black 35 %, scaled 1.02, alpha 0.94)
  that follows the cursor inside its column run (`ListLayout::pane_run`,
  the tiles of the SAME column of the same tab; the travel is clamped to
  the run). The ghost aims at a rank: a row is crossed when the ghost's
  leading edge passes its middle, and crossed back only past its displaced
  middle (a gap of hysteresis, so nothing flickers on the boundary). The
  rows between the origin and the aimed rank step aside by the held height
  plus the 12 pt gap, and a skeleton (a 1 pt dashed rounded outline, white
  25 %, the tile's size and radius, empty inside) marks the aimed spot,
  which is exactly where the tile will land. The aimed rank lives in the
  drag state with the pane id, never a row index: a tick may re-lay the
  list out mid-drag, and the frame is rebuilt from the current layout on
  every event (a gone pane ends the drag, a stale rank is clamped). Drop =
  `sidebar_drop_pane(tab_idx, ids, from, to)`: the adjacent swaps of
  `swap_chain` replayed through `Tab::swap_panes`, then `mark_all_dirty` +
  `resize_all_panes`; a drop at the origin does nothing. Escape while a
  tile is lifted cancels: the rows fall back into place and nothing is
  committed (a local `NSEvent` key monitor installed at lift and removed at
  drop or cancel, since the list never takes first responder). Cross-column
  and cross-tab drops are still v4: Kova has no way to move one pane into
  another tab (`reparent` stays inside a tab, `merge-tab` moves whole tabs).
- Both call `NSView::autoscroll:` on every drag event, so a cursor past the
  visible part keeps the list scrolling.

### 4.3 Menus

Right click on a tile opens an `NSMenu` (`sidebarPaneAction:`, tag =
`PaneAction`, popped up in the list view at the click), items dispatched
through `dispatch_pane_action(pane_id, action)`; the hover buttons and the
tile's own buttons go through the same function.

| Item              | When                                                   | Kova call                                                                 |
|-------------------|--------------------------------------------------------|---------------------------------------------------------------------------|
| Open              | always                                                 | `focus_pane_in_window(id)`                                                |
| Stop              | `is_working()` or a permission prompt is on screen     | `interrupt_pane(id)`: `pane.pty.write(b"\x03")`, `pane.clear_awaiting()`, `set_transient_status("Stopped")` |
| Start Claude here | `Pane::is_bare_shell()` and no resume line             | `pane.pty.write(b"claude\r")`; refused with a status line otherwise      |
| Resume the session | `Pane::restored_session()`                            | the line rebuilt by `agent_session::resume_command` (validated id, the pane's own flags), typed after a Ctrl+U (the pre-typed line may still sit at the prompt) and Enter; refused with a status line otherwise |
| Rename…           | always                                                 | `focus_pane_in_window(id)` then `start_rename_pane()`                     |
| Bookmark / Unbookmark | always                                             | focus then `do_toggle_bookmark()`; label from `bookmark_keys`             |
| Mark read / Mark unread | unread / read                                    | `toggle_pane_unread(id)` (5.2): `Pane::mark_read()` or the manual mark   |
| Minimize / Restore| not minimized / minimized                              | focus then `do_minimize_pane()`; restore = `focus_pane_in_window`         |
| Close             | always                                                 | `ipc_close_pane(id)`; when `is_working()` an `NSAlert` first (`Close {title}?`, `Close and interrupt` / `Keep working`). Refused on the last pane with a status line |

Header menu (`sidebarTabAction:`, tag = `TabAction`): the twelve colours +
`No colour`, separator, `Rename tab…`, `Add a pane` (`do_switch_tab(idx)`
then `do_split(Horizontal)`), `Collapse others`, separator, `Close tab`
(`do_switch_tab(idx)` then `do_close_tab()`, with its confirmation).

Colour picker (`show_sidebar_color_menu`): a click on the header's dot pops
an `NSMenu` under the dot listing the twelve colours (`TAB_COLOR_NAMES`, in
`TAB_COLORS` order: `Red` to `Indigo`, one column), each with a
12 pt filled swatch image (`sidebar_view::swatch_image`, drawn through an
`NSImage` handler so it is sharp on Retina), then `No colour` with a grey
ring, a check mark on the tab's current colour. The items share the header
menu's `sidebarTabAction:` selector and `TabAction::Color(i)` /
`TabAction::NoColor` tags, so a pick runs `run_tab_action`, which sets
`Tab.color` (the same field the tab bar's menu and IPC `set-tab-color`
write); the periodic session save carries it to `session.json` like any
other tab change.

### 4.4 Keys

- `Cmd+J` = `next_attention`, already bound; the pill is its button.
- `Cmd+U` = `toggle_unread` (`[keys]`, default `cmd+u`; also the Pane menu's
  `Mark as Unread` / `Mark as Read`, retitled in `validateMenuItem:`, and
  IPC `dispatch-action` `toggle-unread`): the focused pane becomes read
  when it is unread for any reason, else manually unread (5.2).
- `Cmd+1..9`, `Cmd+Shift+[ ]`, `Cmd+P`, `Cmd+O`: unchanged; the sidebar follows.
- `toggle_sidebar = "cmd+option+s"` (`[keys]`): toggles `layout.mode` and
  persists it. Also reachable over IPC as `dispatch-action` `toggle-sidebar`.

### 4.5 Sort

`⇅ kova` (tab order) or `⇅ activity`: tabs ordered by the most urgent state of
any of their panes, in the tile state's priority order, ties keeping tab
order. Header numbers are the tab's real Cmd+N number either way.

## 5. State mapping (Kova accessors -> tile)

Evaluated once per tick from `Pane` reads (`sidebar_ui::pane_flags`,
`TileState::from_flags`). First match wins; the order doubles as the activity
sort key and the collapsed dot priority.

| # | Condition                                                                                          | Tile     | Glyph / chip                          |
|---|----------------------------------------------------------------------------------------------------|----------|---------------------------------------|
| 1 | `has_permission_prompt()` (a parsed prompt preview) and `!is_working()`                            | awaiting | amber dot, age instead of a chip      |
| 2 | `is_manual_unread()`, or not focused and (`unread_completion()` or bell or `is_awaiting_unseen()` or `is_turn_end_unseen()`) | unread | accent dot, `done` (`bell`, `unread` for the manual mark) chip |
| 3 | `is_working()`                                                                                     | working  | blue dot, `working`                   |
| 4 | `is_starting_agent()`                                                                              | starting | blue dot, `starting`                  |
| 5 | `is_idle_agent()`                                                                                  | idle     | ring, neutral `idle`                  |
| 6 | else                                                                                               | shell    | ring, neutral `shell` and `Start Claude` when bare; neutral `claude` / `codex` and `Resume` on a restored session |

The hook's waiting flag alone (`is_awaiting_unseen()` without a parsed
prompt) paints `done`, never amber: the `Stop` hook raises it at every turn
end (see `docs/ipc.md`), and amber is reserved for a detected permission
prompt, as on the phone. `minimized` adds the `⊟` prefix and the hollow fill
on top of any state. Bell / completion / seen flags clear on focus as before;
the sidebar never acks anything itself.

### 5.2 Unread: one rule for Cmd+J, the pill and the tiles

A pane is UNREAD (`PaneFlags::is_unread`) when something is new since it
was last looked at: an unseen permission prompt (`Pane::is_prompt_unseen`,
the `Permission` preview's `seen` flag), an unseen completion, turn end or
hook notification, a bell, or the manual mark. Looking at a pane means
being the focused pane of the active tab of the key window: the per-frame
pass in `tick.rs` acks the bell and the completion and marks the waiting
flag and the preview seen, so the automatic reasons drain there. Idle
sessions are never unread by themselves (no idle tier any more; `idle`
stays a chip and a summary count).

The manual mark (`Pane::manual_unread`, `Cmd+U`, the tile's `mail` button,
the context menu's `Mark unread`) survives while the pane stays focused: it
is dropped only when the pane BECOMES focused, a transition the frame loop
detects by comparing the focused pane id with the one it saw last
(`KovaView::unread_focus`). `Cmd+U` on an unread pane runs
`Pane::mark_read()` instead: every seen flag set, the manual mark dropped.

`KovaView::collect_unread` lists every pane with its unread bit across
every window: this window's in the sidebar's display order
(`sidebar_model::pane_order`: tabs in kova or activity order, panes in tab
order, minimized panes left out), then the other windows' in tab order. The
pill shows `sidebar::unread_count`; `Cmd+J` and the pill jump to
`sidebar::next_unread(order, focused)`: the first unread pane after the
focused pane's place in that order, wrapping, never the focused pane itself
(a status line says `Nothing to read` when nothing else is). IPC
`list-panes` reports the same rule as `unread`.

### 5.1 Where the question text comes from

Unchanged from v2: `src/prompt_preview.rs` (pure) and `Pane::probe_prompt`
(the trigger) read a Claude pane's screen once its spinner stops (1 s
debounce) and parse the permission prompt with the daemon's grammar; the
fallback reads the last assistant text of the transcript
(`~/.claude/projects/<slug>/<id>.jsonl`) into a `TurnEnd { summary }`.
Cleared on a rising edge of `is_working()` and with the waiting flag in
`Pane::clear_awaiting`. Fixtures under `tests/fixtures/prompt/`.

## 6. Tokens (`sidebar::tokens`, RGB floats from `link/app/src/theme/tokens.ts`)

```
ground           bg.base           #0B0D10
tile             bg.raised         #14171C
tile.hover       bg.overlay        #1B1F26   (also the neutral chip)
tile.pressed     bg.pressed        #1F242B
border.subtle                      #23272F   (= separator, hairlines)
border.strong                      #333944   (ring glyph, pill border)
accent           accent.primary    #4C8DFF   (= unread, focus ring, links)
accent.pressed                     #3A79E6
text.primary                       #E8EAED
text.secondary                     #9BA3AF
text.tertiary                      #7C8593
text.onFill                        #FFFFFF
status.awaiting                    #FFB020
status.awaitingBg                  #2A1F08
status.working                     #38BDF8
status.success                     #3DD68C
status.error                       #FF5C5C
action.interrupt.text              #FF7A7A
tabNone                            #7C8593
tab tint          TAB_COLORS[c] (Kova palette, mirrored by the phone)
panel wash        tint at s -> s x 0.45 (42 %) -> s x 0.14, over bg.raised;
                  s = 0.40 on the selected tab, 0.20 on the others

top 36  summary 24  pill region 36 (pill 28, radius 14, content-sized, right inset 12, pad 12)
list inset 12 (+6 top, +12 bottom)  group gap 16  header 28 (hover pill radius 14)
panel radius 12, pad 12, row gap 12  dot box 16
tile radius 10, pad 8/10  card radius 12, pad 10/12, bar 4
rows: title 20, secondary 16, question 16/line, gap 2  chip 18 (radius 9, pad 7)
hover button 24 (radius 7, gap 2, icon 16)  header + 24 (radius 8)
link icon 11 (gap 4)  pill icon 12 (gap 5)  pill badge 18 (gap 8)  actions row 20  hint 20  width 280, clamp 200..520
```

Motion: hover and pressed states are redrawn on the next frame (KovaLink's
`motion.instant` is under one frame); the caught-up flash lasts 1.6 s; the
working dot does not pulse (a `drawRect:` view; a CA pulse is a candidate for
later).

## 7. Mode switch and persistence

### 7.1 View menu

```
View
  Show Tab Bar            (radio, on when mode = tabs)
  Show Sidebar      ⌥⌘S   (radio, on when mode = sidebar)
```

Both items send `setLayoutMode:` up the responder chain with the mode as tag;
the key window's `KovaView` handles it and refreshes the check mark in
`validateMenuItem:`.

### 7.2 Config (`~/.config/kova/config.toml`)

```toml
[layout]
mode = "sidebar"              # "tabs" | "sidebar", default "tabs"
sidebar_width = 280           # points, clamped to 200..520
sidebar_collapsed_default = false   # new tabs start folded when true

[keys]
toggle_sidebar = "cmd+option+s"
```

Runtime changes (menu, ⌥⌘S, edge drag) are NOT written back
into `config.toml`: they go to `~/.config/kova/prefs.json`
(`{ "mode": "sidebar", "sidebar_width": 300 }`), whose values override the
`[layout]` table on load (`Config::apply_layout_prefs`). Delete the file to
go back to what the TOML says. `sidebar_collapsed_default` is config only.

Per-tab state (collapsed) and the sort mode are session state: `SavedTab`
carries `collapsed` and `WindowSession` carries `sidebar_sort` (`"kova"` |
`"activity"`), saved in `session.json` and restored with the tabs. Prompt
previews are runtime state like the waiting flag and are not saved.

### 7.3 Sync contract

`KovaView::sync_sidebar` runs once per tick, before the renderer lock: it
reads every tab into `TabFacts` (`sidebar_tab_facts`), builds the
`SidebarModel` with Cmd+J's counts and the flash state, and calls
`SidebarView::set_model`, which compares with the previous model and only
then re-lays the list (`ListLayout::new` at the scroll view's width, text
measured with `NSString sizeWithAttributes:` / `boundingRectWithSize:`),
resizes the document view, refreshes the tooltip rects and redraws. Hover,
press and drag state live in the view and redraw on their own.

## 8. Edge and empty states

- One tab, one pane: still a group and one tile, plus the hint.
- Narrow window: the sidebar auto-hides when `window_w - sidebar_w <
  splits.min_width`. The window then renders in tabs mode for as long as it
  is that narrow; `layout.mode` is not touched.
- Second instance without the session lock: one tab, the hint shows.

## 9. Not in v3

- Cross-column / cross-tab pane drop (reparent), `⌥⌘[` `⌥⌘]` collapse /
  expand all, a `Collapse others` shortcut.
- `stale session` badge + `↻ Relaunch`; closed sessions section (⌘O covers
  it); working dot pulse; compact density; collapsed rail.
- Answer buttons inside the awaiting card: never (KovaLink rule A7:
  approving requires reading the prompt in the pane).
