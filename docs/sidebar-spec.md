# Sidebar layout mode

A second way to show a window's tabs: instead of the strip across the top, a
sticky column down the left edge listing every tab as a collapsible group and
every pane as a row with its state at a glance. Mirrors the KovaLink Sessions
list. Everything below is expressed in terminal cell units (`cw` = cell width,
`ch` = cell height, from `renderer.cell_size()`); every row height is
`(k * ch).round()` so glyphs sit on the atlas grid. Only two drawing
primitives are used: filled quads and monospace glyph runs.

Code: `src/window/sidebar.rs` (pure: setting, geometry, hit test, sort, text
rules, unit tests), `src/window/sidebar_ui.rs` (state, mouse, per-frame data),
`Renderer::build_sidebar_vertices` (`src/renderer/mod.rs`).

## 1. Principles

1. The sidebar is a terminal surface, not a Finder clone: the terminal font at
   1x, the tab bar palette, quads. Its one signature is the state square, a
   filled `0.5 cw` square whose colour is the pane's state, the same vocabulary
   as the tab bar dots and the Cmd+J banner.
2. Structure encodes truth. Indentation = "this pane lives in that tab". The
   colour bar on the left of a group = the tab colour the user chose. Numbers on
   headers = the Cmd+N shortcut, and they never change with the sort mode.
3. One click, one meaning. Header click switches tab; chevron click folds; row
   click focuses the pane. No hidden modifier behaviours.
4. Nothing animates; the hover highlight is the only transient effect.
5. Zero cost when off: `layout.mode = "tabs"` keeps the tab bar byte for byte.

## 2. Layout

### 2.1 Window in sidebar mode

```
+---------------------------+-------------------------------------------------+
| ooo             (drag)    | column 0              | column 1        | col 2 |  top: 2 ch
+---------------------------+                       |                 |       |
| 1 waiting · 2 working  kova|                       |                 |       |  summary: 1.5 ch
+---------------------------+                       |                 |       |
|▾ 1  link                  |                       |                 |       |  header 1.5 ch
|   ■  fix-voice-input      |                       |                 |       |  pane row 2.5 ch
|      claude · ~/link      |                       |                 |       |
|   □  zsh                  |                       |                 |       |
|      ~/link/app           |                       |                 |       |
|▸ 2  kova            ■ 3   |                       |                 |       |  collapsed
|▾ 3  perso                 |                       |                 |       |
|   ■  daemon rewrite       |  <- list scrolls      |                 |       |
|      codex · ~/perso      |     vertically        |                 |       |
|                           |                       |                 |       |
| Kova v1.9.0     « tab bar |                       |                 |       |  footer 1.5 ch
+---------------------------+-------------------------------------------------+
| global status bar (full window width, unchanged)                             |  1 ch
+-----------------------------------------------------------------------------+
```

`ooo` = the macOS traffic lights; they live inside the sidebar's top area. The
1 px (scaled) column at the sidebar's right edge is the separator and the
resize handle. Panes start at `x = sidebar_w + sep_w`, `y = 0`: there is no
tab bar over the content area (`tab_bar_height()` is 0). The "content
viewport" (`KovaView::content_viewport`) is everything right of the sidebar;
a tab's virtual width and horizontal scroll are measured against its width,
not the window's.

The sidebar lists the tabs of its own window only, like the tab bar.

### 2.2 Width

Resizable by dragging the separator, snapped to whole cells.

- Default `28` cells, clamped to `[18, 48]` (`config::SIDEBAR_WIDTH_RANGE`).
  Stored in cells so it keeps the same apparent size across font sizes and
  displays.
- Separator: `round(1 px * scale)`, colour `[0.20, 0.20, 0.23]`. Hit tolerance
  `4 pt * scale` either side, cursor `resizeLeftRight`.
- Drop rule: `cells = (px / cw).round().clamp(18, 48)` on every drag event;
  panes are resized live; persisted on mouse up.

### 2.3 Vertical regions (top to bottom)

| Region      | Height   | Content                                                  |
|-------------|----------|----------------------------------------------------------|
| Top area    | `2 ch`   | Traffic lights, window drag region, double click = zoom  |
| Summary row | `1.5 ch` | Left: "N waiting · M working"; right: sort toggle        |
| List        | rest     | Groups and rows, vertical scroll                         |
| Footer      | `1.5 ch` | Left: "Kova vX.Y.Z" dim; right: "« tab bar" button       |

The global status bar (1 ch) stays full window width below both. Text in the
summary row and footer is vertically centred. The summary row is always shown
(it also carries the sort toggle).

### 2.4 Group header (one per tab), `1.5 ch`

```
x (in cw):  0    1    2    3    4 ..................... W-5   W-1
            |bar| ▾  | 1  |    | title (truncated)       |■ 3  |
```

- Colour bar `round(0.25 cw)` wide at `x = 0` on the header and every row of
  the group: `TAB_COLORS[color]`, `dim_inactive_tab()` on a non-active tab.
  Absent when the tab has no colour.
- Chevron at cell 1: `▾` expanded, `▸` collapsed, `tab_bar.fg_color`.
- Tab number at cell 2 (two digits spill into cell 3): `tab_bar.fg_color`,
  `[0.80, 0.80, 0.85]` on the active tab.
- Title from cell 4 to `W-1` (expanded) or `W-5` (collapsed). Active tab
  `[1, 1, 1]`, others `[0.72, 0.72, 0.78]`.
- Collapsed summary, right aligned ending at cell `W-1`: `[square] count`.
  Square amber if any pane is awaiting, else blue if any is working, else
  omitted; count = number of panes, `[0.45, 0.45, 0.50]`.
- Active tab header background: `tab_bar.active_bg` across the full width.
  Hover `[0.16, 0.16, 0.19]`, pressed `[0.19, 0.19, 0.22]`.

### 2.5 Pane row, `2.5 ch`, two text lines

```
x (in cw):  0    1    2    3    4 ....................................... W-1
line 1:     |bar|    | sq |    | title (truncated with …)                   |
line 2:     |bar|    |    |    | secondary (dim)                            |
```

- `line1_y = row_y + 0.25 ch`, `line2_y = row_y + 1.25 ch`.
- State square: side `s = round(0.5 cw)`, centred in cell 2 on line 1. Hollow
  variant = four `1 px * scale` quads on the same box.
- Title from cell 4 to `W-1`: `pane.display_title("shell")` (agent name >
  custom title > OSC title > process > cwd basename), activity marker
  stripped. Focused pane of the active tab `[1, 1, 1]`; other panes of the
  active tab `[0.80, 0.80, 0.85]`; panes of other tabs `[0.60, 0.60, 0.66]`. A
  minimized pane's title is prefixed by `⊟ `.
- Secondary line, `[0.45, 0.45, 0.50]`: `"{agent} · {cwd_short}"` when the pane
  runs Claude or Codex, else `"{fg_process} · {cwd_short}"`, else `cwd_short`.
  `cwd_short` = the OSC 7 cwd with `$HOME` folded to `~`, tail-truncated. A
  bookmarked pane paints its secondary line in `colors.paste_block` blue.
- Row background: focused pane of the active tab `tab_bar.active_bg`, hover
  `[0.16, 0.16, 0.19]`, pressed `[0.19, 0.19, 0.22]`, spanning `0.25 cw .. W`.

### 2.6 State square (priority order, first match wins)

| Condition (from `Pane`)     | Square         | RGB                  |
|-----------------------------|----------------|----------------------|
| `is_awaiting()`             | filled, amber  | `[1.00, 0.69, 0.13]` |
| bell (unread)               | filled, orange | `[1.00, 0.45, 0.10]` |
| `unread_completion()`       | filled, green  | `[0.20, 0.80, 0.30]` |
| `is_working()`              | filled, blue   | `[0.22, 0.74, 0.97]` |
| `is_idle_agent()`           | hollow, grey   | `[0.50, 0.50, 0.55]` |
| shell, no agent             | none           |                      |

Amber and blue are the KovaLink `status.awaiting` / `status.working` tokens.
Bell and completion never paint on the focused pane of the active tab.

### 2.7 List geometry

- `gap = round(0.5 ch)` after the last row of an expanded group; none after a
  collapsed header.
- `list_y = 3.5 ch`, `list_h = sidebar_h - 3.5 ch - 1.5 ch` where
  `sidebar_h` = window height minus the global bar.
- Content height = headers + rows + gaps (+ one header height for the hint).
  `scroll_y` clamped to `[0, max(0, content_h - list_h)]`. Rows are clipped to
  the list region by geometry (quads cut, a text line that would overflow is
  skipped), not by a scissor.
- Draw order: panes first (clipped by their own viewports), then the sidebar
  with an opaque ground, then the global bar. Painting the sidebar last is what
  makes it sticky: a column scrolled under it is simply covered. Hit tests
  reject `px < sidebar_w + sep_w` before consulting `tab.hit_test`.

### 2.8 Truncation (char based, never byte slices)

- Titles: as is when `chars <= n`, else the first `n - 1` chars and `…`.
- Paths: from the left, `…` and the last chars, advanced to the next `/` so
  the line starts on a segment boundary (`…/personal-tools/kova`).
- Rename in progress: the edit buffer with its last `n` chars visible and a
  `▏` cursor glyph (same rule as the tab bar).

## 3. Interaction

### 3.1 Mouse

| Target                  | Click                                                  | Double click | Right click     |
|-------------------------|--------------------------------------------------------|--------------|-----------------|
| Header, cells 0..2      | Toggle collapse (no tab switch)                         |              | tab colour menu |
| Header, cells 2..W      | switch tab; on the active tab: toggle collapse          | rename tab   | tab colour menu |
| Pane row                | switch to its tab, focus it, reveal it, restore if minimized | rename pane |            |
| Sort toggle             | kova ↔ activity                                         |              |                 |
| "« tab bar" footer      | switch `layout.mode` to tabs                            |              |                 |
| Top area                | window drag                                             | zoom         |                 |
| Separator ±4 pt         | drag resizes the sidebar                                |              |                 |

Pressed state paints on mouse down, the action fires on mouse up inside the
same target; a drag that leaves the target cancels the click. Hover is
tracked in `mouseMoved`; only the row under the cursor repaints differently.

### 3.2 Wheel

Over the sidebar, the vertical delta scrolls the list (trackpad:
`dy * scale * scroll_sensitivity / 6`; mouse wheel: `dy * ch` per notch). The
horizontal delta is dropped: the sidebar never forwards wheel events to the
panes. On every focus change (Cmd+J, Cmd+P, click, Cmd+N, IPC) the list
scrolls the minimum amount that shows the focused pane's row; if its group is
collapsed the header is what gets revealed (the group is not auto-expanded).

### 3.3 Tab drag reorder

Mouse down on a header (cells 2..W), move 3 px: the header lifts (background
`[0.19, 0.19, 0.22]`, title white) and follows the cursor as a floating copy
drawn last. An insertion line, `2 px * scale` tall, `splits.focus_border_color`,
spans the sidebar between the two groups the cursor is over (midpoint rule:
above a header's vertical centre inserts before it). The drop goes through the
same code as the `move-tab` IPC command; the active tab keeps its identity.
While the cursor is within `1.5 ch` of the list's top or bottom edge the list
scrolls one header height every 100 ms. Drag is disabled in activity sort.

### 3.4 Keys

- `Cmd+1..9`, `Cmd+Shift+[`/`]`, `Cmd+J`, `Cmd+P`: unchanged; the sidebar follows.
- `toggle_sidebar = "cmd+option+s"` (new `[keys]` entry): toggles `layout.mode`
  between `tabs` and `sidebar` and persists it. Also reachable over IPC as
  `dispatch-action` `toggle-sidebar`.

### 3.5 Sort

`kova` (tab order, dim label) or `activity` (blue label): tabs ordered by the
most urgent state of any of their panes, in the square's priority order, ties
keeping tab order. Header numbers are the tab's real Cmd+N number either way.

## 4. Tokens

```
sidebar.bg              = tab_bar.bg_color        [0.12, 0.12, 0.14]
sidebar.separator                                 [0.20, 0.20, 0.23]
sidebar.row.active_bg   = tab_bar.active_bg       [0.22, 0.22, 0.26]
sidebar.row.hover_bg                              [0.16, 0.16, 0.19]
sidebar.row.pressed_bg                            [0.19, 0.19, 0.22]
sidebar.text.primary                              [1.00, 1.00, 1.00]
sidebar.text.active_tab                           [0.80, 0.80, 0.85]
sidebar.text.other_tab                            [0.60, 0.60, 0.66]
sidebar.text.header                               [0.72, 0.72, 0.78]
sidebar.text.dim        = tab_bar.fg_color        [0.50, 0.50, 0.55]
sidebar.text.secondary                            [0.45, 0.45, 0.50]
sidebar.text.bookmark   = colors.paste_block      [0.60, 0.80, 1.00]
sidebar.insertion       = splits.focus_border     [0.40, 0.60, 1.00]
summary.text            = state.awaiting when N waiting > 0, else state.working

top_h = round(2 ch)   summary_h = header_h = footer_h = round(1.5 ch)
row_h = round(2.5 ch) group_gap = round(0.5 ch)   text_col = 4 cw
color_bar_w = round(0.25 cw)   square = round(0.5 cw)
sidebar_w = cells * cw, cells in [18, 48], default 28   sep_w = round(scale)
```

Typography: the terminal font at 1x everywhere. The only glyphs outside ASCII
are `▾ ▸ … · ⊟ « ⌘ ▏`, rasterised on demand by the atlas.

## 5. Mode switch and persistence

### 5.1 View menu

```
View
  Show Tab Bar            (radio, on when mode = tabs)
  Show Sidebar      ⌥⌘S   (radio, on when mode = sidebar)
```

Both items send `setLayoutMode:` up the responder chain with the mode as tag;
the key window's `KovaView` handles it and refreshes the check mark in
`validateMenuItem:`. The shortcut shown is the configured `toggle_sidebar`
binding; the key itself is handled in `performKeyEquivalent` like every other
Kova binding (and toggles), the menu only ever sees it if no Kova window is key.

### 5.2 Config (`~/.config/kova/config.toml`)

```toml
[layout]
mode = "sidebar"              # "tabs" | "sidebar", default "tabs"
sidebar_width = 28            # cells, clamped to 18..48
sidebar_collapsed_default = false   # new tabs start folded when true

[keys]
toggle_sidebar = "cmd+option+s"
```

Runtime changes (menu, ⌥⌘S, edge drag, footer button) are NOT written back
into `config.toml`: rewriting the user's TOML would need a comment-preserving
editor, which is not worth a dependency. They go to
`~/.config/kova/prefs.json`:

```json
{ "mode": "sidebar", "sidebar_width": 30 }
```

Values present there override the `[layout]` table on load
(`Config::apply_layout_prefs`); delete the file to go back to what the TOML
says. `sidebar_collapsed_default` is config only.

Per-tab state (collapsed) and the sort mode are session state: `SavedTab`
carries `collapsed` and `WindowSession` carries `sidebar_sort` (`"kova"` |
`"activity"`), saved in `session.json` alongside everything else and restored
with the tabs. Tab ids are not stable across launches, so collapse is
persisted inside the saved tab rather than keyed by id.

### 5.3 Geometry contract

`SidebarGeometry::new(cell, scale, width_cells, height, scroll_y, kinds,
show_hint)` in `src/window/sidebar.rs` lays out a list of `SidebarRowKind`
(`Header { tab_idx, collapsed }` | `Pane { tab_idx, pane_id }`) and answers
`hit(px, py) -> Option<SidebarHit>` where `SidebarHit = Chevron(tab) |
Header(tab) | Pane(pane_id) | SortToggle | ModeButton | Edge | TopArea |
Empty`, plus `reveal`, `insertion_index`, `insertion_line_y`,
`autoscroll_direction` and `overflow`. The renderer and the mouse handlers both
consume it (`KovaView::sidebar_geometry`), rebuilt from the tab list on demand.

## 6. Edge and empty states

- One tab, one pane: still a header and one row. Below the last group, when the
  window has fewer than 3 panes in total, a dim hint at `text_col`:
  `⌘T new tab · ⌘D split`. It disappears at 3 panes.
- More rows than height: wheel scroll; no scrollbar. Hidden overflow is
  signalled by a `1 px * scale` line in `sidebar.separator` colour at the top
  edge of the list when `scroll_y > 0` and at the bottom edge when more content
  is below.
- Narrow window: the sidebar auto-hides when
  `window_w - sidebar_w - sep_w < splits.min_width * scale` (300 pt by default).
  The window then renders in tabs mode for as long as it is that narrow;
  `layout.mode` is not touched and the sidebar comes back on the first resize
  that makes room. The window's minimum size stays 200 x 150 pt.
- Second instance without the session lock: one tab, the hint shows.

## 7. Not in v1

- Pane row drag reorder (column-major mapping to swap / reparent).
- Right click context menu on a pane row.
- Collapse / expand all, ⌥⌘[ and ⌥⌘].
- Working square breathing.
- Collapsed rail for narrow windows; compact single-line rows.
- Tooltip with the full title and cwd on hover.
