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
  arithmetic (`swap_chain`, `drop_index`), the text rules. Unit tests.
- `src/window/sidebar_model.rs`: pure. `SidebarModel { summary, sort, pill,
  groups: Vec<GroupVm { tiles: Vec<TileVm> }>, show_hint }`, built from
  `TabFacts` / `PaneFacts` (plain reads of the tabs). `PartialEq`, so the view
  can tell whether a tick changed anything. Unit tests.
- `src/window/sidebar_view.rs`: the AppKit side. `ListLayout` and
  `ChromeLayout` (pure geometry in points, hit tests, drag slots; text widths
  through the `TextMetrics` trait, unit tests with a stand-in), then two
  `NSView` subclasses drawn with `drawRect:`: `SidebarView` (chrome: top
  area, summary row, Next pill, footer, resize edge, and an `NSScrollView`)
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
   Close, Rename, Bookmark, Minimize / Restore, Start Claude, Add a pane.
4. Native where it is free: SF Pro, `NSScrollView` (elastic, overlay
   scroller), `scrollRectToVisible`, `autoscroll:`, tooltip rects, cursor
   rects, `NSMenu`, `NSAlert`.
5. Zero cost when off: in tabs mode the sidebar view is hidden and
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
| Pill     | 36     | The Next pill, 28 tall, radius 14, inset 12                     |
| List     | rest   | `NSScrollView`, width = inner width - 4 (the edge zone stays the chrome's) |
| Footer   | 24     | 1 pt top hairline; `Kova vX.Y.Z` left; `« Tab bar` button right  |

### 3.3 Typography (`sidebar_view::Style`, `NSFont::systemFontOfSize_weight`)

KovaLink's scale two steps down for a Mac list: header and tile titles 13
semibold; secondary line, detail, summary, links, hint, number 11 (links
semibold); chips, sort toggle, footer, pill badge 10 (chips and sort medium,
badge bold); question and pill label 12 (pill semibold); `+` 14 medium.
Truncation is AppKit's: titles at the tail, paths and rename edit buffers at
the head (so the `▏` cursor stays visible), the question wrapped to two lines
with an ellipsis on the last (`TruncatesLastVisibleLine`).

### 3.4 Group (one per tab; `TabGroupView.tsx`)

Content inset 12 on each side; groups 16 apart; a group is a block with a
3 pt tint bar down its left edge (clipped to a radius of 6), alpha 1 on the
active tab, 0.55 on the others; content starts 12 pt after the bar. Header
28 tall, radius 6: chevron (open `▾` / folded `▸`, secondary), 8 pt tint dot,
the Cmd+N number in tertiary, the title (13 semibold; in the tint on the
active tab, whose header is filled with the tint at 15 %, or `bg.overlay`
without a colour), then at the right the `+` round button (22 pt, `bg.raised`,
`bg.pressed` on hover) and, when folded, the collapsed summary: a 6 pt amber
dot when a pane awaits, a 6 pt blue dot when one works, `4 panes` in
tertiary. Hover: white at 6 %; pressed 12 %. The tint is `TAB_COLORS[c]` or
`tabNone` grey.

### 3.5 Tile (`SessionRow.tsx`), radius 10, fill `bg.raised`, padding 8 v / 10 h

Row 1 (16 tall): the state glyph at x 10 (an 8 pt dot in the state colour;
a 1.5 pt `border.strong` ring for idle and shell), the title from x 26 (`⊟ `
prefix when minimized), the chip right-aligned: 18 tall, radius 9, padding 7,
`workingBg` fill + working text (`working`, `starting`), `accent.subtleBg`
fill + accent text + 6 pt accent dot (`done`, `bell`), `bg.overlay` fill +
tertiary text (`idle`, `shell`). Row 2 (14 tall, 2 below): `claude · ~/cwd`
in secondary, a `★` in amber first when bookmarked; a bare shell shows
`▶ Start Claude` (accent link, primary on hover) right-aligned instead of the
end of the path. An unread tile with a turn-end summary adds a third row in
secondary. Heights 48 / 64. Focused pane of the active tab: fill
`bg.overlay` and a 1.5 pt accent ring. Hover: `bg.overlay`; pressed:
`bg.pressed`. Minimized: fill `bg.base`, 1 pt `border.subtle`.

Hover actions replace the chip on row 1, right to left: `×` close (error
red on hover), `⊟` / `⊞` minimize / restore, `■` stop (interrupt colour;
working or awaiting), `▶` start Claude (accent; bare shell). Each is a 20 pt
round button (`bg.pressed`, a `border.strong` ring on hover), 4 apart, with a
tooltip (`Close`, `Minimize`, `Restore`, `Stop`, `Start Claude here`) through
`addToolTipRect:owner:userData:`. Pressed paints on mouse down, the action
fires on mouse up inside the same button (a leave cancels).

### 3.6 Awaiting card (`AwaitingCard.tsx`), radius 12, fill `awaitingBg`

A 4 pt amber bar at the left edge (clipped by the radius), a 1 pt amber
border at 35 % (80 % on hover), padding 10 v / 12 h. Row 1: amber dot, title,
the age (`4m`) right in tertiary, `status.error` past 10 min. Then the
question (12, primary, 1 or 2 lines, measured), the detail (11, secondary:
the command, the file, or the header), and a 20 pt actions row: `Open`
(accent pill, white 11 semibold, `primaryPressed` on hover) at the text
column, `■ Stop` (interrupt colour, primary on hover) at the right. Heights
77 (1 line, no detail) to 108.

### 3.7 Summary, sort, Next pill

- Summary runs: `N waiting` amber, `N working` blue (starting counts as
  working), `N idle` tertiary, dots tertiary; zero counts omitted; all zero:
  `nothing running`. Counts cover this window; the pill counts cover every
  window (like Cmd+J).
- Sort toggle: `⇅ kova` tertiary / `⇅ activity` accent, primary on hover on
  a `bg.pressed` rounded ground.
- Next pill (`sidebar::NextPill`, from KovaLink `NextPill.tsx`):

| State     | Condition                             | Fill                             | Text                         | Badge                          |
|-----------|---------------------------------------|----------------------------------|------------------------------|--------------------------------|
| next      | Cmd+J's unread tier is not empty      | `accent.primary`                 | white `▶ Next unread`        | white 18 pt circle, accent digits |
| idle      | unread empty, idle tier not           | `bg.overlay` + 1 pt `border.strong` | secondary `▶ Next idle`   | `bg.pressed` circle, secondary digits |
| caught up | both empty, unread just dropped to 0  | `bg.overlay`                     | `status.success` `✓ All caught up` | none; 1.6 s, then `nothing` |
| nothing   | both empty                            | `bg.overlay`                     | tertiary `✓ Nothing to read` | none, not clickable            |

  `⌘J` right-aligned inside at 60 % alpha, the badge to its left. Hover:
  `accent.primaryPressed` (next) / `bg.pressed` (idle). Pressed: text alpha
  0.85. The tiers come from `KovaView::collect_attention`
  (`src/window/attention.rs`), the same collection `do_focus_next_attention`
  jumps with, read once per tick.

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
| Header chevron zone (24 pt)| toggle collapse, no tab switch                         | same               | header menu  |
| Header body                | `do_switch_tab(idx)`; on the active tab: toggle collapse | `start_rename_tab` | header menu |
| Header `+`                 | add a pane (4.4)                                       |                    | header menu  |
| Tile body                  | `focus_pane_in_window(id)` (switches tab, restores, reveals) | rename pane  | tile menu    |
| Tile hover buttons         | the action, without focusing the pane                  |                    | tile menu    |
| Awaiting `Open`            | same as tile body                                      |                    | tile menu    |
| Awaiting `■ Stop`          | interrupt (4.4)                                        |                    | tile menu    |
| Shell `▶ Start Claude`     | start Claude (4.4)                                     |                    | tile menu    |
| Next pill                  | `do_focus_next_attention()` (not when `nothing`)       |                    |              |
| Sort toggle                | kova <-> activity                                      |                    |              |
| `« Tab bar` footer         | switch `layout.mode` to tabs                           |                    |              |
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
- Pane drag: mouse down on a tile body, move 3 pt: the tile lifts the same
  way and the line, at the tile's width, appears between tiles of the SAME
  column of the same tab (`ListLayout::pane_run`). Drop = `sidebar_drop_pane`:
  the adjacent swaps of `swap_chain` replayed through `Tab::swap_panes`, then
  `mark_all_dirty` + `resize_all_panes`. A drop more than a header height
  above or below the run snaps back. Cross-column and cross-tab drops are
  still v4.
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
| Start Claude here | `Pane::is_bare_shell()`                                | `pane.pty.write(b"claude\r")`; refused with a status line otherwise      |
| Rename…           | always                                                 | `focus_pane_in_window(id)` then `start_rename_pane()`                     |
| Bookmark / Unbookmark | always                                             | focus then `do_toggle_bookmark()`; label from `bookmark_keys`             |
| Minimize / Restore| not minimized / minimized                              | focus then `do_minimize_pane()`; restore = `focus_pane_in_window`         |
| Close             | always                                                 | `ipc_close_pane(id)`; when `is_working()` an `NSAlert` first (`Close {title}?`, `Close and interrupt` / `Keep working`). Refused on the last pane with a status line |

Header menu (`sidebarTabAction:`, tag = `TabAction`): the six colours +
`No colour`, separator, `Rename tab…`, `Add a pane` (`do_switch_tab(idx)`
then `do_split(Horizontal)`), `Collapse others`, separator, `Close tab`
(`do_switch_tab(idx)` then `do_close_tab()`, with its confirmation).

### 4.4 Keys

- `Cmd+J` = `next_attention`, already bound; the pill is its button.
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
| 2 | not focused and (`unread_completion()` or bell or `is_awaiting_unseen()` or `is_turn_end_unseen()`) | unread | accent dot, `done` (`bell`) chip     |
| 3 | `is_working()`                                                                                     | working  | blue dot, `working`                   |
| 4 | `is_starting_agent()`                                                                              | starting | blue dot, `starting`                  |
| 5 | `is_idle_agent()`                                                                                  | idle     | ring, neutral `idle`                  |
| 6 | else                                                                                               | shell    | ring, neutral `shell`, `▶ Start Claude` when bare |

The hook's waiting flag alone (`is_awaiting_unseen()` without a parsed
prompt) paints `done`, never amber: the `Stop` hook raises it at every turn
end (see `docs/ipc.md`), and amber is reserved for a detected permission
prompt, as on the phone. `minimized` adds the `⊟` prefix and the hollow fill
on top of any state. Bell / completion / seen flags clear on focus as before;
the sidebar never acks anything itself.

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
accent.subtleBg                    #12213A   (unread chip)
text.primary                       #E8EAED
text.secondary                     #9BA3AF
text.tertiary                      #7C8593
text.onFill                        #FFFFFF
status.awaiting                    #FFB020
status.awaitingBg                  #2A1F08
status.working                     #38BDF8
status.workingBg                   #0A1F2B   (working chip)
status.success                     #3DD68C
status.error                       #FF5C5C
action.interrupt.text              #FF7A7A
tabNone                            #7C8593
tab tint          TAB_COLORS[c] (Kova palette, mirrored by the phone)

top 36  summary 24  pill region 36 (pill 28, radius 14)  footer 24
list inset 12 (+6 top, +12 bottom)  group gap 16  tile gap 6  header 28 (radius 6)
group bar 3 + inset 12  tile radius 10, pad 8/10  card radius 12, pad 10/12, bar 4
rows: title 16, secondary 14, question 15/line, gap 2  chip 18 (radius 9, pad 7)
hover button 20 (gap 4)  actions row 20  hint 20  width 280, clamp 200..520
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

Runtime changes (menu, ⌥⌘S, edge drag, footer button) are NOT written back
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
