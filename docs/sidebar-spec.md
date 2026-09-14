# Sidebar layout mode (v2)

A second way to show a window's tabs: instead of the strip across the top, a
sticky column down the left edge listing every tab as a coloured band and
every pane as a tile with its state at a glance, the way the KovaLink home
screen does it. v2 replaces the dim text list of v1 with tiles, chips, a
summary line and a Next pill, and adds the phone's actions (stop, close,
rename, bookmark, minimize, start Claude, add a pane) one click away.

Everything below is expressed in terminal cell units (`cw` = cell width, `ch`
= cell height, from `renderer.cell_size()`); every y and height is
`(k * ch).round()` so glyphs sit on the atlas grid. Only two drawing
primitives are used: filled quads (with alpha) and monospace glyph runs. No
radius, no images, no bold: weight comes from fills, bars and chips.

Code: `src/window/sidebar.rs` (pure: setting, tokens, geometry, hit test,
tile states, chips, buttons, pane drag arithmetic, text rules, unit tests),
`src/window/sidebar_ui.rs` (state, mouse, actions, context menus, per-frame
data), `Renderer::build_sidebar_vertices` (`src/renderer/mod.rs`),
`src/prompt_preview.rs` (the permission-prompt parser and the turn-end
summary), `Pane::probe_prompt` (`src/pane.rs`).

## 1. Principles

1. Readable from across the room. The state must be legible at 2 m (a solid
   colour bar and an inverted chip per tile, a solid colour band per tab), the
   title at 1 m.
2. Same structure and vocabulary as KovaLink: tab groups, tiles, `waiting` /
   `working` / `starting` / `done` / `idle` / `shell` chips, the awaiting tile
   with its question, the Next pill, the sort toggle, the collapsed summary,
   the `+` on a group. Same colour tokens (`sidebar::tokens`).
3. Same actions as the phone's swipes and sheets, one click away: Open, Stop,
   Close, Rename, Bookmark, Minimize / Restore, Start Claude, Add a pane.
4. Everything v1 got right stays: sticky, resizable, wheel scroll, collapse
   persisted per tab, tab drag reorder, narrow fallback, View menu, ⌥⌘S, pure
   `SidebarGeometry` with unit tests.
5. Zero cost when off: `layout.mode = "tabs"` keeps the tab bar byte for byte.
   The darker ground is the sidebar's only; the tab bar and status bar are
   untouched.

## 2. Layout

### 2.1 Window in sidebar mode (W = 32 cells)

```
+---------------------------------+-----------------------------------------+
| ooo                             | column 0              | column 1        |  2 ch
| 2 waiting · 3 working · 4 idle ⇅|                       |                 |  1.5 ch
|  ▶ Next unread          [3] ⌘J  |  <- accent fill, white text            |  2 ch
|▾ 1  Claap                     + |  <- solid tab colour band (active tab) |  2 ch
| ▌fix-voice-input            4m  |                       |                 |
| ▌Should I overwrite hello.txt   |  <- awaiting tile, 5 ch, amber ground  |
| ▌with the new content?          |                       |                 |
| ▌Open ⏎              ■ Stop     |                       |                 |
| ▌daemon rewrite      [working]  |  <- working tile, 3 ch                 |
| ▌claude · ~/link/daemon         |                       |                 |
| ▌zsh                  [shell]   |  <- bare shell tile, 3 ch              |
| ▌~/link/app       ▶ Start Claude|                       |                 |
|▸ 2  Perso          [1] 4 panes +|  <- collapsed, tinted band, chips      |  2 ch
|▾ 3  Marketing                 + |                       |                 |
| ▌claude              [● done]   |  <- unread tile, 4 ch                  |
| ▌claude · ~/Claap/Marketing     |                       |                 |
| ▌Pushed the landing page copy   |  <- turn-end summary, 1 line           |
| Kova v1.12.0          « tab bar |                       |                 |  1.5 ch
+---------------------------------+-----------------------------------------+
| global status bar                                                         |  1 ch
+---------------------------------------------------------------------------+
```

`ooo` = the macOS traffic lights; they live inside the sidebar's top area.
`▌` = the tile's state bar. `[...]` = a filled chip with inverted text
(bracket and box characters are notation, chips are quads). The `1 px`
(scaled) column at the sidebar's right edge is the separator and the resize
handle. Panes start at `x = sidebar_w + sep_w`, `y = 0`: there is no tab bar
over the content area. The sidebar lists the tabs of its own window only.

### 2.2 Width

Resizable by dragging the separator, snapped to whole cells.

- Default `32` cells, clamped to `[22, 56]` (`config::SIDEBAR_WIDTH_RANGE`).
- Separator: `round(1 px * scale)`, `border.subtle`. Hit tolerance
  `4 pt * scale` either side, cursor `resizeLeftRight`.
- Drop rule: `cells = (px / cw).round().clamp(22, 56)` on every drag event;
  panes are resized live; persisted on mouse up.

### 2.3 Vertical regions (top to bottom)

| Region      | Height   | Content                                                       |
|-------------|----------|---------------------------------------------------------------|
| Top area    | `2 ch`   | Traffic lights, window drag region, double click = zoom       |
| Summary     | `1.5 ch` | Left: `2 waiting · 3 working · 4 idle`; right: `⇅ kova` / `⇅ activity` |
| Next pill   | `2 ch`   | A `1.5 ch` button with `0.25 ch` margins, `x = 1 cw .. W - 1 cw` |
| List        | rest     | Groups: header `2 ch`, tiles, gaps; vertical scroll           |
| Footer      | `1.5 ch` | Left: `Kova vX.Y.Z` dim; right: `« tab bar` button            |

`list_y = 5.5 ch`, `list_h = sidebar_h - 5.5 ch - 1.5 ch`. The global status
bar (1 ch) stays full window width below both.

### 2.4 Group header (one per tab), `2 ch`, full width

```
x (cw):  0   1   2   3   4   5 ................... W-12  W-4  W-3  W-1
         |   | ▾ |   | 1 |   | title (truncated)     |chips|  + |   |
text_y = y + 0.5 ch
```

- Band fill: active tab `TAB_COLORS[c]`; other tabs `band_tint(dim_inactive_tab(c))`
  = `ground + (dim(c) - ground) * 0.22` (KovaLink `${tint}22`). No colour:
  active `bg.overlay`, others `bg.raised`.
- Text on an active band: `on_band(c)` = `text.inverse` when the band's
  luminance is above 0.55 (yellow, green), white otherwise; without a colour,
  `text.primary`. Other tabs: chevron, number, title in `dim_inactive_tab(c)`
  (no colour: `text.secondary`).
- Number = the Cmd+N slot, never the sort rank. Title from cell 5, ending
  `W - 4` cells in when expanded; when collapsed the chips take what they
  need first (`CollapsedSummary::cells`) and the title gets the rest.
- `+` at cell `W - 3`, hit zone `W - 4 .. W - 1`: the band's text colour at
  70 %, full on hover with a white 12 % box behind it. Action: add a pane.
- Collapsed summary, right-aligned ending at cell `W - 4`: chip `[n]` amber
  when n panes are awaiting, chip `[n]` working blue when n are working,
  then `k panes` (`1 pane`) in the text colour at 75 %. Expanded: nothing.
- Hover: white 6 % overlay on the band. Pressed: 12 %.
- Spacing: `0.5 ch` above every tile (after the header and between tiles),
  `1 ch` between the last tile and the next header, nothing after a
  collapsed header.

### 2.5 Pane tile

```
tile_x = 1 cw     tile_w = (W - 2) cw     bar_w = round(0.5 cw)
content_x = tile_x + 1.5 cw     content_right = tile_x + tile_w - 1 cw
line k y = tile_y + round((0.5 + k) ch)     tile_h = (lines + 1) ch
```

| Kind                 | lines | Line 0                         | Line 1                                    | Lines 2, 3                                   |
|----------------------|-------|--------------------------------|-------------------------------------------|----------------------------------------------|
| agent (idle/working) | 2     | title + chip                   | `claude · ~/cwd` secondary                |                                              |
| unread (done / bell) | 3     | title + `[● done]` accent chip | `claude · ~/cwd`                          | turn-end summary, 1 line, secondary (2 lines when there is none) |
| bare shell           | 2     | title + `[shell]` neutral chip | `~/cwd` + `▶ Start Claude` accent, right  |                                              |
| awaiting             | 4     | title + age right (`4m`)       | question line 1, primary                  | question line 2 (or the detail, secondary) / `Open ⏎` accent + `■ Stop` interrupt red |
| minimized            | as above | `⊟ title` + chip            |                                           | tile fill = ground, border only              |

- Fill `bg.raised`; border `1 px * scale` `border.subtle` (four quads inside
  the tile bounds). The state bar covers the left border. Awaiting: fill
  `status.awaitingBg`, border `status.awaiting` at alpha 0.35 (0.8 on hover).
- State bar colour = the chip colour; neutral states use `border.strong` so
  every tile has a bar.
- Chip: quad `(chars + 2) cw` wide, `1 ch` tall, right-aligned at
  `content_right`, text inset `1 cw`, fill = state colour, text
  `text.inverse`; neutral chip fill `bg.pressed`, text `text.secondary`. Chip
  copy: `working`, `starting`, `● done`, `● bell`, `idle`, `shell`. The
  awaiting tile shows its age instead of a chip.
- Title: `pane.display_title("shell")`, `text.primary` on every tab (the band
  already says which tab is active), truncated to the room left of the chip,
  the age or the glyph boxes.
- Line 1: `secondary_line(agent, process, cwd_short)` as v1, `text.secondary`;
  a bookmarked pane paints it `accent.primary`.
- Focused pane of the active tab: ring `2 px * scale` `border.focus` over
  the border, fill `bg.overlay`.
- Hover: fill `bg.overlay` and the quick actions replace the chip on line 0
  (3.1). Pressed: `bg.pressed`.
- Awaiting age: `now - since` as `Ns` / `Nm` / `Nh` in `text.tertiary`;
  older than 10 min: `status.error` (KovaLink `aging`). Actions line:
  `Open ⏎` in `accent.primary` at `content_x`, `■ Stop` in
  `action.interrupt.text` right-aligned; both `1 ch` tall click targets,
  `text.primary` on hover.
- Question preview: word-wrapped into at most 2 lines of
  `content_right - content_x` cells (`wrap_text`), the last line ending with
  `…` when cut. Line 2 shows the detail (the command, the file name; the
  header when there is none) only if the question took one line. Turn-end
  summary on unread tiles: the first non-empty line of Claude's last answer,
  markdown marks stripped, tail-truncated with `…` (`summary_line`).

### 2.6 Summary, sort, Next pill

- Summary runs: `N waiting` in `status.awaiting`, `N working` in
  `status.working` (starting panes count as working), `N idle` in
  `text.tertiary`, `·` in tertiary. Zero counts are omitted; all zero:
  `nothing running`. Counts cover this window (like the list); the Next pill
  counts cover every window (like Cmd+J).
- Sort toggle: `⇅ kova` in tertiary, `⇅ activity` in `accent.primary`,
  `text.primary` on hover. Hit zone: the last 12 cells of the summary row.
- Next pill states (`sidebar::NextPill`, from KovaLink `NextPill.tsx`):

| State     | Condition                             | Fill                                 | Text                              | Badge                              |
|-----------|---------------------------------------|--------------------------------------|-----------------------------------|------------------------------------|
| next      | Cmd+J's unread tier is not empty      | `accent.primary`                     | white `▶ Next unread`             | white chip, accent digits          |
| idle      | unread empty, idle tier not           | `bg.overlay` + 1 px `border.strong`  | secondary `▶ Next idle`           | `bg.pressed` chip, secondary digits|
| caught up | both empty, unread just dropped to 0  | `bg.overlay`                         | `status.success` `✓ All caught up` | none; 1.6 s, then `nothing`       |
| nothing   | both empty                            | `bg.overlay`                         | tertiary `✓ Nothing to read`      | none, not clickable                |

  `⌘J` right-aligned inside the pill in the text colour at alpha 0.6, the
  badge to its left. Hover: `accent.primaryPressed` (next) / `bg.pressed`
  (idle). Pressed: same plus text alpha 0.85. The tiers come from
  `KovaView::collect_attention` (`src/window/attention.rs`), the same
  collection `do_focus_next_attention` jumps with, rebuilt once per frame.

### 2.7 List geometry

- Content height = headers + tiles + gaps (+ one header height for the hint).
  `scroll_y` clamped to `[0, max(0, content_h - list_h)]`. Rows are clipped
  to the list region by geometry (quads cut, a text line that would overflow
  is skipped), not by a scissor.
- Draw order: panes first (clipped by their own viewports), then the sidebar
  with an opaque ground, then the global bar. Painting the sidebar last is
  what makes it sticky. Hit tests reject `px < sidebar_w + sep_w` before
  consulting `tab.hit_test`.

### 2.8 Truncation (char based, never byte slices)

- Titles: as is when `chars <= n`, else the first `n - 1` chars and `…`.
- Paths: from the left, `…` and the last chars, advanced to the next `/` so
  the line starts on a segment boundary (`…/personal-tools/kova`).
- Questions: `wrap_text`, on spaces, a word longer than a line split.
- Rename in progress: the edit buffer with its last `n` chars visible and a
  `▏` cursor glyph (same rule as the tab bar).

## 3. Interaction

### 3.1 Mouse

| Target                            | Click                                                        | Double click       | Right click  |
|-----------------------------------|--------------------------------------------------------------|--------------------|--------------|
| Header cells 0..3 (chevron)       | toggle collapse, no tab switch                               | same               | header menu  |
| Header cells 3..W-4               | `do_switch_tab(idx)`; on the active tab: toggle collapse     | `start_rename_tab` | header menu  |
| Header `+` (W-4..W-1)             | add a pane (3.4)                                             |                    | header menu  |
| Tile body                         | `focus_pane_in_window(id)` (switches tab, restores, reveals) | rename pane        | tile menu    |
| Tile hover glyphs (line 0)        | the action, without focusing the pane                        |                    | tile menu    |
| Awaiting `Open ⏎`                 | same as tile body                                            |                    | tile menu    |
| Awaiting `■ Stop`                 | interrupt (3.4)                                              |                    | tile menu    |
| Shell `▶ Start Claude`            | start Claude (3.4)                                           |                    | tile menu    |
| Next pill                         | `do_focus_next_attention()` (not when `nothing`)             |                    |              |
| Sort toggle                       | kova ↔ activity                                              |                    |              |
| `« tab bar` footer                | switch `layout.mode` to tabs                                 |                    |              |
| Top area                          | window drag                                                  | zoom               |              |
| Separator ±4 pt                   | drag resizes the sidebar                                     |                    |              |

Hover glyph buttons, right-aligned on line 0 in place of the chip, each a
`3 cw` hit box with a `bg.overlay` quad (`bg.pressed` while pressed), the
glyph in `text.secondary`, `text.primary` on hover, `action.interrupt.text`
for `■`, `status.error` for `×`. Shown only when they apply, in this order
from the right: `×` close, `⊟` minimize / `⊞` restore, `■` stop (working or
awaiting), `▶` start Claude (bare shell). Each box is a tooltip zone
(`Close`, `Minimize`, `Restore`, `Stop`, `Start Claude here`) through the
renderer's `push_tooltip_zone`, read back in `sidebar_mouse_moved`. Pressed
paints on mouse down, the action fires on mouse up inside the same box (a
leave cancels), as v1. The hit test knows the boxes whether or not the tile
is hovered, since a mouse over them means it is.

### 3.2 Wheel, reveal

Over the sidebar, the vertical delta scrolls the list (trackpad:
`dy * scale * scroll_sensitivity / 6`; mouse wheel: `dy * ch` per notch). The
horizontal delta is dropped. On every focus change the list scrolls the
minimum amount that shows the focused pane's tile; if its group is collapsed
the header is what gets revealed.

### 3.3 Drag

- Tab drag: as v1. Mouse down on a header, move 3 px: the header lifts (its
  band at alpha 0.9, a floating copy drawn last) and an insertion line
  `2 px * scale` `border.focus` spans the sidebar between the two groups the
  cursor is over (midpoint rule). The drop goes through the `move-tab` IPC
  path. Auto-scroll within `1.5 ch` of the list edges. Disabled in activity
  sort.
- Pane drag (new): mouse down on a tile body, move 3 px: the tile lifts
  (alpha 0.9, drawn last) and an insertion line indented to `tile_x` appears
  between tiles of the SAME column of the same tab (`SidebarGeometry::pane_run`:
  the sidebar order is column-major, so the candidates are the contiguous run
  of tiles sharing the pane's column). Drop = the adjacent swaps of
  `swap_chain` replayed through `Tab::swap_panes` (the primitive behind the
  `swap-pane` IPC command), then `mark_all_dirty` + `resize_all_panes`. A
  drop more than `1 ch` above or below the run snaps back. Cross-column and
  cross-tab drops are v3.

### 3.4 Actions and the Kova functions behind them

Right click on a tile opens an `NSMenu` (`sidebarPaneAction:`, tag =
`PaneAction`), items dispatched through `dispatch_pane_action(pane_id, action)`;
the hover glyphs and the tile's own buttons go through the same function.

| Item              | When                                                   | Kova call                                                                 |
|-------------------|--------------------------------------------------------|---------------------------------------------------------------------------|
| Open              | always                                                 | `focus_pane_in_window(id)`                                                |
| Stop              | `is_working()` or a permission prompt is on screen     | `interrupt_pane(id)`: `pane.pty.write(b"\x03")`, `pane.clear_awaiting()` (drops the prompt preview), `set_transient_status("Stopped")` |
| Start Claude here | `Pane::is_bare_shell()`: no agent, no foreground process, nothing pending | `pane.pty.write(b"claude\r")`; refused with a status line otherwise (the daemon's 409). The tile shows `starting` until the session resolves |
| Rename…           | always                                                 | `focus_pane_in_window(id)` then `start_rename_pane()`                     |
| Bookmark / Unbookmark | always                                             | focus then `do_toggle_bookmark()`; label from `bookmark_keys`             |
| Minimize / Restore| not minimized / minimized                              | focus then `do_minimize_pane()`; restore = `focus_pane_in_window`, which restores |
| Close             | always                                                 | `ipc_close_pane(id)`; when `is_working()` an `NSAlert` first: `Close {title}?`, `The agent is working right now: closing interrupts the task.`, `Close and interrupt` / `Keep working`. Refused on the last pane with a status line |

Header menu (`sidebarTabAction:`, tag = `TabAction`): the six colours +
`No colour`, separator, `Rename tab…` (`start_rename_tab`), `Add a pane`
(`do_switch_tab(idx)` then `do_split(Horizontal)`, the ⌘D path: side by
side), `Collapse others`, separator, `Close tab` (`do_switch_tab(idx)` then
`do_close_tab()`, with its confirmation).

### 3.5 Keys

- `Cmd+J` = `next_attention`, already bound; the pill is its button.
- `Cmd+1..9`, `Cmd+Shift+[ ]`, `Cmd+P`, `Cmd+O`: unchanged; the sidebar follows.
- `toggle_sidebar = "cmd+option+s"` (`[keys]`): toggles `layout.mode` and
  persists it. Also reachable over IPC as `dispatch-action` `toggle-sidebar`.

### 3.6 Sort

`⇅ kova` (tab order) or `⇅ activity`: tabs ordered by the most urgent state of
any of their panes, in the tile state's priority order, ties keeping tab
order. Header numbers are the tab's real Cmd+N number either way.

## 4. State mapping (Kova accessors -> tile)

Evaluated per frame from `Pane` reads (`sidebar_ui::pane_state`,
`TileState::from_flags`). First match wins; the order doubles as the activity
sort key and the collapsed chip priority.

| # | Condition                                                                                          | Tile     | Bar / chip                           |
|---|----------------------------------------------------------------------------------------------------|----------|--------------------------------------|
| 1 | `has_permission_prompt()` (a parsed prompt preview) and `!is_working()`                            | awaiting | amber, age instead of a chip         |
| 2 | not focused and (`unread_completion()` or bell or `is_awaiting_unseen()` or `is_turn_end_unseen()`) | unread | accent `● done` (`● bell` for a bell) |
| 3 | `is_working()`                                                                                     | working  | blue `working`                       |
| 4 | `is_starting_agent()`: a pending restore command, or `claude` in the foreground without a session  | starting | blue `starting`                      |
| 5 | `is_idle_agent()`                                                                                  | idle     | `border.strong`, neutral `idle`      |
| 6 | else                                                                                               | shell    | `border.strong`, neutral `shell`, `▶ Start Claude` when bare |

The hook's waiting flag alone (`is_awaiting_unseen()` without a parsed
prompt) paints `● done`, never amber: the `Stop` hook raises it at every
turn end (see `docs/ipc.md`), and amber is reserved for a detected permission
prompt, as on the phone. `minimized` adds the `⊟` prefix and the hollow fill
on top of any state. Bell / completion / seen flags clear on focus as before
(`ack_completion`, `mark_awaiting_seen`, `mark_idle_agent_seen`,
`mark_turn_end_seen`); the sidebar never acks anything itself.

### 4.1 Where the question text comes from

Kova has no prompt text of its own: `is_awaiting()` is the hook claim set over
`set-pane-status`. KovaLink's daemon reads the screen instead
(`daemon/src/prompt/detector.ts`), and v2 ports that to Rust in
`src/prompt_preview.rs` (pure) and `Pane::probe_prompt` (the trigger):

- `Pane.prompt_preview: RefCell<Option<PromptPreview>>`, with
  `Permission { header, question, detail, since }` and
  `TurnEnd { summary, seen }`. Runtime state, never saved.
- Trigger: `probe_prompt` runs from `Tab::check_running` on every tick and
  follows `is_working()`. On a falling edge it arms a 1 s debounce; when it
  fires and the pane is still not working and `agent_kind() == Some(Claude)`,
  it reads `terminal.dump_text(DumpMode::Visible, true)` once (the
  `get-pane-content` path) and runs `parse_permission_prompt`. The grammar is
  the daemon's, observed on Claude Code 2.1.268, not invented: a `─` frame
  line, a header line (`Bash command`, `Create file`), detail lines, the
  question ending with `?`, numbered options from 1 (`❯ 1. Yes`), and
  `Esc to cancel` as the last non-empty line. Any deviation yields `None`.
- Fallback: `transcript_path(home, cwd, agent_session_id)` =
  `~/.claude/projects/<slug>/<id>.jsonl` (the daemon's `projectSlug`), the
  last 64 KB read from a line boundary, the last `assistant` record's last
  non-empty `text` block -> `TurnEnd { summary }` (`turn_end_summary`).
- Cleared: a rising edge of `is_working()` (Claude got its answer), and with
  the waiting flag in `Pane::clear_awaiting` (a keystroke into the pane,
  `send-keys`, `interrupt_pane`, the shell back at a bare prompt, pane
  death). `TurnEnd.seen` is set on the focused pane of the key window, in
  the same place as `mark_awaiting_seen`.
- Tests: the daemon's fixtures copied under `tests/fixtures/prompt/`
  (`prompt-bash`, `prompt-bash-consecutive-1/2`, `prompt-write`,
  `screen-idle` and `screen-trust-dialog` -> `None`, `transcript-echo.jsonl`),
  asserted verbatim in `src/prompt_preview.rs`.

## 5. Tokens (`sidebar::tokens`, RGB floats from `link/app/src/theme/tokens.ts`)

```
ground           bg.base           [0.043 0.051 0.063]   #0B0D10
tile             bg.raised         [0.078 0.090 0.110]   #14171C
tile.hover       bg.overlay        [0.106 0.122 0.149]   #1B1F26
tile.pressed     bg.pressed        [0.122 0.141 0.169]   #1F242B
border.subtle                      [0.137 0.153 0.184]   #23272F   (= separator)
border.strong                      [0.200 0.224 0.267]   #333944
border.focus     accent.primary    [0.298 0.553 1.000]   #4C8DFF   (= unread)
accent.pressed                     [0.227 0.475 0.902]   #3A79E6
text.primary                       [0.910 0.918 0.929]   #E8EAED
text.secondary                     [0.608 0.639 0.686]   #9BA3AF
text.tertiary                      [0.486 0.522 0.576]   #7C8593
text.inverse                       [0.043 0.051 0.063]   #0B0D10
text.onFill                        [1.000 1.000 1.000]
status.awaiting                    [1.000 0.690 0.125]   #FFB020
status.awaitingBg                  [0.165 0.122 0.031]   #2A1F08
status.working                     [0.220 0.741 0.973]   #38BDF8
status.success                     [0.239 0.839 0.549]   #3DD68C
status.error                       [1.000 0.361 0.361]   #FF5C5C
action.interrupt.text              [1.000 0.478 0.478]   #FF7A7A
tab band          TAB_COLORS[c] (Kova palette); tint = ground + (dim(c) - ground) * 0.22

top_h = 2 ch   summary_h = 1.5 ch   pill_h = 1.5 ch (+0.25 ch margins)   header_h = 2 ch
tile_h = 3 / 4 / 5 ch   tile_gap = 0.5 ch   group_gap = 1 ch   footer_h = 1.5 ch
tile_x = 1 cw   bar_w = 0.5 cw   content_x = 2.5 cw   chip_h = 1 ch   glyph_box = 3 cw
border = 1 px * scale   focus_ring = 2 px * scale   width 32 cells, clamp 22..56
```

Typography: the terminal font at 1x only. Hierarchy = primary / secondary /
tertiary greys plus inverted chips. Non-ASCII glyphs used:
`▾ ▸ ▶ ■ × ⊟ ⊞ ● ✓ ⇅ ⌘ ⏎ … · « ▏`, rasterised on demand by the atlas.

## 6. Mode switch and persistence

### 6.1 View menu

```
View
  Show Tab Bar            (radio, on when mode = tabs)
  Show Sidebar      ⌥⌘S   (radio, on when mode = sidebar)
```

Both items send `setLayoutMode:` up the responder chain with the mode as tag;
the key window's `KovaView` handles it and refreshes the check mark in
`validateMenuItem:`.

### 6.2 Config (`~/.config/kova/config.toml`)

```toml
[layout]
mode = "sidebar"              # "tabs" | "sidebar", default "tabs"
sidebar_width = 32            # cells, clamped to 22..56
sidebar_collapsed_default = false   # new tabs start folded when true

[keys]
toggle_sidebar = "cmd+option+s"
```

Runtime changes (menu, ⌥⌘S, edge drag, footer button) are NOT written back
into `config.toml`: they go to `~/.config/kova/prefs.json`
(`{ "mode": "sidebar", "sidebar_width": 30 }`), whose values override the
`[layout]` table on load (`Config::apply_layout_prefs`). Delete the file to
go back to what the TOML says. `sidebar_collapsed_default` is config only.

Per-tab state (collapsed) and the sort mode are session state: `SavedTab`
carries `collapsed` and `WindowSession` carries `sidebar_sort` (`"kova"` |
`"activity"`), saved in `session.json` and restored with the tabs. Prompt
previews are runtime state like the waiting flag and are not saved.

### 6.3 Geometry contract

`SidebarGeometry::new(cell, scale, width_cells, height, scroll_y, kinds,
show_hint)` in `src/window/sidebar.rs` lays out a list of `SidebarRowKind`
(`Header { tab_idx, collapsed }` | `Pane { tab_idx, pane_id, column, tile:
TileLayout }`) and answers `hit(px, py) -> Option<SidebarHit>` where
`SidebarHit = Chevron(tab) | Header(tab) | HeaderAdd(tab) | Pane(pane_id) |
PaneButton(pane_id, TileButton) | SortToggle | NextPill | ModeButton | Edge |
TopArea | Empty`, plus `reveal`, `insertion_index`, `insertion_line_y`,
`pane_run`, `pane_insertion_slot`, `pane_insertion_line_y`,
`autoscroll_direction`, `overflow`, and the tile metrics (`tile_x`, `tile_w`,
`bar_w`, `content_x`, `content_right`, `line_y`, `glyph_boxes`). The renderer
and the mouse handlers both consume it (`KovaView::sidebar_geometry`),
rebuilt from the tab list on demand.

## 7. Edge and empty states

- One tab, one pane: still a band and one tile. Below the last group, when
  the window has fewer than 3 panes in total, a dim hint: `⌘T new tab · ⌘D split`.
- More rows than height: wheel scroll; no scrollbar. Hidden overflow is
  signalled by a `1 px * scale` line in `border.strong` at the top edge of the
  list when `scroll_y > 0` and at the bottom edge when more content is below.
- Narrow window: the sidebar auto-hides when
  `window_w - sidebar_w - sep_w < splits.min_width * scale`. The window then
  renders in tabs mode for as long as it is that narrow; `layout.mode` is not
  touched.
- Second instance without the session lock: one tab, the hint shows.

## 8. Not in v2 (v3)

- Cross-column / cross-tab pane drop (reparent), `⌥⌘[` `⌥⌘]` collapse /
  expand all, a `Collapse others` shortcut.
- `stale session` badge + `↻ Relaunch` (needs the child-process probe the
  daemon has).
- Closed sessions section with `Resume` (⌘O already covers it).
- Working bar breathing behind a config flag; compact density (2 ch tiles);
  collapsed rail.
- Answer buttons inside the awaiting tile: never (KovaLink rule A7:
  approving requires reading the prompt in the pane).
