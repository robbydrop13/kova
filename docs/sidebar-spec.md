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
- `src/window/sidebar_asks.rs`: pure layout, drawing and hit test of the Asks
  list (cards, answers, snooze, the folded snoozed section), plus `AskQueue`,
  the queue the app delegate drains into `ask::Board`. Unit tests.
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

The window's content view is a plain container holding three siblings: the
`SidebarView` at the left, the `KovaView` (Metal) in the middle, and the side
panel at the right, hidden until something is opened in it
(`docs/side-panel.md`).
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
| Top row  | 38     | Traffic lights, window drag (`performWindowDragWithEvent`), double click = zoom; the view switcher right-aligned 10 from the inner edge |
| Title    | 40     | Left, 14 in: the list's name (`Style::ViewTitle`); right, 12 from the inner edge: the Next pill, 24 tall |
| Meta     | 20     | Inside the list inset and 6 more: the summary on the left, the sort ghost on the right |
| List     | rest   | `NSScrollView` down to the bottom, width = inner width - 4 (the edge zone stays the chrome's) |

The sidebar spans the window's full height, under its title bar: the top row
is where macOS draws the traffic lights, and the switcher shares it at the
right end rather than sitting on a row of its own.

No footer: the way back to the tab bar is View > Show Tab Bar and ⌥⌘S
(section 7); the version label stays in the tab bar of tabs mode.

### 3.3 Typography (`sidebar_view::Style`, `fonts::inter_name`, `NSFont::fontWithName_size`)

The design system's type scale, in Inter: the chrome's view title 15
semibold; header and tile titles 13 medium; secondary line, question, detail
and pill label 12 (detail regular, pill semibold); links 12 medium; chips,
summary, hint, number and sort ghost 11 (chips and sort medium); pill badge
10 bold. A plus sign drawn as a glyph stays at 14 medium.

The four faces (Regular, Medium, SemiBold, Bold) are bundled in the binary
(`assets/fonts/Inter-*.ttf`, SIL Open Font License 1.1) and registered with
CoreText at launch (`src/window/fonts.rs`, once per process, already
registered counting as success); every style asks CoreText for its PostScript
name and falls back to the system font (`systemFontOfSize:weight:`) if the
name does not resolve. Text colours are the phone's: primary `#E8EAED`,
secondary `#9BA3AF`, tertiary `#7C8593`.
Truncation is AppKit's: titles and subtitles at the tail (never a head-cut
`..jects/Acme`), rename edit buffers at the head (so the `▏` cursor stays
visible), the question wrapped to two lines with an ellipsis on the last
(`TruncatesLastVisibleLine`).

### 3.4 Group (one block per tab)

Content inset 12 on each side; groups 8 apart (`GROUP_GAP`). A group draws
nothing of its own: the selected one is a `bg1` card, radius 8, with a 1 pt
`border.subtle` stroke, and it pads 4 pt above its header and under its last
tile (`GROUP_PAD_V`). The whole block is one click target: a click inside it
but outside a row (that 4 pt padding, the gaps, the 8 pt side insets, the
empty bottom) is `ListHit::Panel` and selects the tab, and the hand cursor
covers every block (cursor rects on the list view, refreshed on relayout).
Every row of a group is inset by the same `ROW_INSET` (8 pt) from the card's
edges, the header included, so their grounds line up; the tiles are 4 pt
apart (`ROW_GAP`). No
tint bar in the panel: the tab's colour is the bar in the header, drawn from
`TAB_COLORS[c]` (twelve colours: red, orange, yellow, green, blue, violet,
pink, coral, lime, teal, cyan, indigo) or `tabNone` grey.

Header 28 tall, its hover ground a rounded row (radius 6), no ground of its
own otherwise (the card shows through): the `chevron-down` / `chevron-right`
icon (12 pt, in a 16 pt zone from the row's left edge), the tab's colour as
a filled dot 8 pt across (`DOT_D`, `ListLayout::dot_rect`) in the tab's
colour 0..11, the title (`Style::Title`, 13
medium, primary), then at the right the pane count (`Style::Number`, 11,
tertiary) and, when folded, the 6 pt amber / blue dots of the panes that wait
or work, left of the count. Row hover: white 5 %; pressed 10 %. The `+` (24
pt, radius 8, 8 pt in from the row's right edge) takes the count's place
while the mouse is anywhere on the header row (dot and `+` included): the
count is not drawn then, so the two never share the column, whose right edge
is the tiles' one. The `+` has no ground of its own, and white 14 % (radius
8) appears under it only while the mouse is on the `+` itself (20 %
pressed). The colour dot is a button (section 4.3, colour picker): its
16 pt box (`ListLayout::dot_button`, the dot itself in
`ListLayout::dot_rect`, both centred 22 pt into the row by
`ListLayout::dot_centre`) is its hit (`ListHit::HeaderDot`), between the
chevron zone and the title, and a 1 pt white ring 12 pt across at 20 %
(35 % pressed) appears around it under the mouse. `ListLayout::header_chevron`
is the chevron's 12 pt box.

#### 3.4.1 Selected tab

The selected tab's card differs by its fill and border, and by its text:

- Header: title white; count, chevron and folded dots white 72 %
  (secondary / tertiary elsewhere). The `+` is white when shown (primary
  elsewhere).
- Tiles: no ground of their own, they sit on the card; under the mouse a
  `bg.hover` ground (`tokens::get().tile_hover`, `gray.1400` `#3A3A3A`),
  radius 6, and `btn2.hover` pressed. The focused pane is a **neutral**
  `gray.1500` ground (`#2F2F2F`, `tokens::get().sel_row_bg`), never a filled
  blue row, with a 2 x 16 pt `blue.400` rail (`#7498FF`,
  `tokens::get().sel_row_rail`) 4 pt in from its left edge as the selection
  marker; its title, subtitle and meta column are `gray.25` (`#FAFAFA`,
  `tokens::get().sel_row_text`) and its idle / shell ring follows the same
  ink. The state dot keeps its own colour, and the unread dot stays the
  accent blue. In the light set the ground is `gray.100` (`#E9E9E9`), the ink
  `gray.1600` (`#242424`) and the rail `blue.500` (`#4C74FF`).

### 3.5 Tile, radius 6, no fill, padding 4 v / 8 h

One line: a 26 pt row (`TILE_HEIGHT`: 4 + the 18 pt name line + 4, the same
for every pane whatever it holds), inset `ROW_INSET` 8 pt from the group's
edges like the header:

- Left: a 16 pt box at 8 pt from the tile's edge (`TILE_PAD_H`) holding the
  8 pt state dot in the state colour; idle and shell draw a 1.5 pt
  `text.tertiary` ring instead of a fill.
- From 32 pt into the tile (`TEXT_X`; 40 pt into the group, aligned with the
  header's title): the session name (`Style::Title`, 13 medium, one line,
  tail-truncated, `⊟ ` prefix when minimized, a `★` in amber first when
  bookmarked). The folder and the agent are no longer printed under it: the
  name line's tooltip carries what the subtitle said (`TileVm::tooltip`), so
  `install_tooltips` puts a zone over it.
- Right, 8 pt in (`TILE_PAD_H`, the same edge as the header's count and `+`):
  the right column (`tile_column`), one of three, the first that applies:
  the wait (`waiting 4m` in `warn.text`, then `· Answer`, an accent link hit
  target `ListHit::NeedAnswer`), the call to action (`Start Claude` on a bare
  shell, `Resume` on a restored session, each after a filled 11 pt `play`,
  accent link, `text.primary` on the focused tile), or the state word
  (`Style::Number`, 11: `working` / `starting`). A 6 pt accent dot comes
  before it on an unread tile.
- The hover buttons (`close`, `minimize`, `mark unread`, `stop`) sit
  `COLUMN_GAP` (8 pt) left of the column, never over it: a call to action or
  an `Answer` link stays clickable while the mouse is on the row. Hovering
  never hides the name: a wait shows only `Answer` while hovered (the
  `waiting 4m ·` age gives way), and only the buttons that leave the name
  `TITLE_MIN_W` (72 pt) show, the leftmost giving way first (`stop`, then
  `mark unread`, …; `close` last). The name truncates with an ellipsis.

The awaiting pane is no longer a tall card: same 26 pt row, amber dot and
the `waiting 4m · Answer` column. Its question is not drawn in the row (the
`Needs you` block takes it), and neither is the turn-end summary.


Under the mouse the meta column gives way to the hover actions, right to
left: `x` close (error red on hover), `minimize-2` / `maximize-2` minimize /
restore, `check` mark read (unread tile) or `mail` mark unread (read tile),
and `square` stop (interrupt colour; working or awaiting only). No play
glyph: `Start Claude` and `Resume` are the links on the subtitle line, and a
click on the tile already focuses the pane. Each is a 22 pt button (radius 6,
bare; white 12 % under the one hovered, 20 % pressed) holding a 16 pt Feather
icon, 2 apart, on the title line, with a tooltip (`Close`, `Minimize`,
`Restore`, `Mark read`, `Mark unread`, `Stop`) through
`addToolTipRect:owner:userData:`. Pressed paints on mouse down, the action
fires on mouse up inside the same button (a leave cancels).

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
  phone's `projectName`), the agent left out when it is already the title or
  when it is `claude` (the default agent: naming it is noise, other agents
  stay), and the project alone on a plain shell.
- Agent: the live one (`Pane::agent_kind()`), else the one whose resume line
  waits at the prompt (`Pane::restored_session()`: a bare shell whose last
  command is `claude … --resume <id>` or `codex resume <id>`, which is what a
  restored pane holds until Enter is pressed). Such a restored session keeps
  the shell tile (ring) but titles itself after its agent (`claude` /
  `codex`) and its call is `Resume`; `Shell` and `Start Claude` are for a
  plain shell only.
- The group header without a custom tab name is titled the same way from the
  focused pane.

#### Icon set (`src/window/feather.rs`)

Feather icons, drawn as stroked `NSBezierPath`s on Feather's 24 pt grid
(2 pt stroke, round caps and joins) mapped into a 16 pt box centred in the
button, so the stroke is 1.33 pt and the icon scales with the row. The
views are flipped (y down, like SVG), so the published coordinates are used
as they are. `chevron-down` / `chevron-right` (collapse caret), `plus`
(header), `x` (close), `maximize-2` / `minimize-2` (restore / minimize),
`square` (stop), `check` / `mail` (mark read / unread). The `Start Claude`
and `Resume` links and the Next pill use the filled `play` (11 pt in the
links, 12 pt in the pill). `Icon::segments` and the point mapping are pure
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

- Summary runs, on the meta row: `N working` in `status.infoText` (starting
  counts as working), the waiting and idle counts and the dots tertiary;
  zero counts omitted; all zero: `nothing running`. Counts cover this
  window; the pill counts cover every window (like Cmd+J).
- Sort ghost, at the meta row's right: the `sort` icon at 11 then the
  label, `Tab order` in the default order and `Activity` when the list is
  sorted by urgency. Tertiary, `ACCENT` while sorted by activity, primary
  under the mouse, on a 10 % white ground (radius 4). The Panes list only:
  Asks, Agents and Scheduled draw no ghost and have no sort hit target.
- Next pill (`NextPill`, `sidebar.rs`), one state pair (section 5.2):

| State   | Fill             | Text                                   | Badge |
|---------|------------------|----------------------------------------|-------|
| next    | `ACCENT`         | white play 11 + `Next`                 | white circle 16, accent count |
| nothing | `BG2`            | tertiary `Nothing to read`             | none, and not a button |

  The pill is 24 tall and content-sized (`pill_width`: 9 pt padding each
  side, the 11 pt play and a 4 pt gap when clickable, the label, a 6 pt gap
  and the badge when there is a count; the circle is 16 wide at least,
  wider for two digits). It sits on the title row, 12 from the inner right
  edge; the row's title keeps an 8 pt gap, so a long name truncates before
  the two meet. `Nothing` draws no play and no badge, ignores hover, and
  its hit does nothing. `Next` darkens to `ACCENT_PRESSED` under the mouse;
  pressed, its text and icon drop to 85 %. What it shows follows `Cmd+J`
  (`sidebar_model::next_pill`): `Next` as soon as a need item has a pane to
  land on, badged with what waits on you (`waits_on_you`: asks and
  permissions, at least 1 while any need pane exists); otherwise `Next` with
  the unread count; `Nothing to read` only when neither exists.

### 3.7.1 View switcher

Four buttons on the top row, right-aligned 10 from the inner edge, on the
traffic lights' own row: 26 pt each, 2 apart, radius 6 (`SWITCH_LISTS`).
`sidebar` shows the panes, `message` the asks, `users` the agents,
`clock` the scheduled routines. The icons are 15 pt strokes: the selected
one sits on `SEL_BG` in `SEL_TEXT`, the others are `TEXT_TERTIARY` and take
a 10 % white ground under the mouse (16 % while pressed). A press on a
button switches lists; the rest of the row still drags the window.
Clicking the selected icon does nothing, like a segmented control.

Two badges, both hung on the icon's top right corner (4 pt in from its
right edge, 5 pt above its top) with a 2 pt ring of the sidebar ground:

- Asks: the count of waiting questions in `ACCENT`, white 10 bold digits,
  15 pt at least and wider for two digits. Shown when `asks_waiting > 0`.
- Scheduled: an 8 pt `ERROR` dot when `routine_failed`.

### 3.7.2 Scheduled list (`SidebarList::Scheduled`)

`src/window/sidebar_sched.rs` owns this list: the view state, the pure layout,
the drawing and the hit test. The routines come from `crate::routines`; the
rows are grouped into sections, next run first, and each row can run, pause,
resume, edit or delete its routine through `routinectl` (3.7.2.3).

The meta row reads `N active · K failed` on the left (`failed` in `crit`,
`K` counting `NeedsAttention`) and the clock the list is phrased against on
the right, `now 18:45`.

#### Sections

| Section | Holds | Note | Folded at start |
|---|---|---|---|
| `Needs attention` | the last run failed, and it is not paused | | no |
| `Today` | next run later today | `next first` | no |
| `This week` | next run within seven days | | no |
| `Later` | further out, and manual routines | | no |
| `Off` | `enabled: false` | `paused` | **yes** |

A header is 24 pt: the chevron (12), the label (11 medium, `crit` on
`Needs attention`), the count in tertiary, and the note right-aligned.
Clicking it folds or unfolds the section; the folded state lives in
`SidebarState.sched` (`folded`, `Off` in it by default) and rides the model.
An empty section is not drawn. Rows inside a section sort by next run, a
manual routine having none and coming last, then by name.

#### A row

40 pt, 5 v / 6 h padding, radius 6, `bg.hover` under the mouse; the open row
is `bg1` with a 1 pt `b3` border at radius 8. An `Off` row is drawn at alpha
0.55: switched off on purpose is not broken, and a routine switched off and
forgotten is exactly what this view is for.

| Part | Content | Style |
|---|---|---|
| Glyph (16 wide) | ok `check` in `ok`; failed `x-circle` in `crit`; running an 8 pt `info` dot; never run a `ring` in tertiary; off `power` in tertiary | |
| Name | the routine's name | `Title` (13 medium) primary, truncated |
| Line 2 | `<schedule words> · ran 10 min ago` / `· failed 2 h ago` (crit) / `· running` (info) / `· never run`; off: `Paused · was <schedule words>` | `Secondary` (12) |
| Meta | next run (`19:05`, `Wed 09:03`, `9 Nov`) over `in 20 min`; nothing for `Off` and manual | `Secondary` in secondary / `Number` (11) tertiary |

Under the mouse the meta gives way to the row's ghost icon buttons (22 pt,
radius 6, icon 14): `Run now` (play), `Open log` (file-text), `Pause` (pause),
`Edit prompt` (edit), `Delete` (trash, crit). An off row keeps two:
`Resume` (play) and `Delete`.

Clicking the row opens its body (one at a time, `open_routine`), indented 24:

| Key (72 pt) | Value |
|---|---|
| `Runs at` | the schedule in words (`2 times a day`) |
| `Next` | `Wed 09:03 · in 2 d`, `Manual`, or `Paused` |
| `Last run` | `Ok · 10 min ago`, `Failed 2 h ago · exit 1` (crit), `Never`, `Running` |
| `Where` | the working directory, its home written `~` |
| `Prompt` | `prompts/<name>.md` |

Then the same actions as icon buttons, on the hover row's metrics: 22 pt,
radius 6, a 14 pt glyph (`Run now` play, `Open log` file-text, `Pause`
pause, `Edit prompt` edit, `Delete` trash), `bg.hover` ground at rest, white
16 % under the mouse and the press, 2 pt between buttons, wrapped onto as
many lines as the width needs. The words ride on the tooltip
(`SchedButton::tip`, answered by `stringForToolTip`) instead of being
printed: `Run now`, `Open log`, `Pause`, `Resume`, `Edit prompt`, `Delete
routine`. `Delete` is the exception: no ground and crit glyph, so the button
that only opens the confirmation does not wear the colour of the one that
deletes. `body_height` is shared by the layout and the draw so the two
cannot disagree; the body is carried in the model only while the row is open
(`cwd` is emptied otherwise), so a closed list stays as cheap to compare on
the tick as it was.

#### 3.7.2.1 Deleting

`Delete` opens an inline confirmation in the body: a crit wash (radius 6,
padding 8 10), `Delete <name>?` in bold over `Removes its schedule and its
prompt`, then `Cancel` (ghost, unchanged) and `Delete` filled solid
`crit.solid` (`#ED3726`, `#CC2314` under the mouse) in white 11 semibold.
Escape or `Cancel` closes it; a click on the row does too. Nothing is deleted
until the filled button is clicked.

#### 3.7.2.2 Running, failing

While an action runs the row's second line says what it is doing
(`Running now…`, `Pausing…`, `Resuming…`, `Deleting…`, tertiary) and its
buttons ignore clicks until the outcome lands. A failure keeps
`Could not <verb>: <first line of stderr, 80 chars>` in crit on the second
line for 10 s (`ERROR_MS`), then it goes.

#### 3.7.2.3 Actions

`routines::run_action` runs `~/.claude/routines/routinectl run|disable|enable|
delete <name>` on a background thread, argv only, no shell, and refuses any
name outside `^[a-z0-9][a-z0-9-]*$`. When it ends it forces the snapshot
stale (`refresh`) and leaves an `ActionResult` in a queue that the tick
drains (`sync_sched_results`, the same shape as `AskQueue`).

`Open log` runs `open -a Console <log>` and `Edit prompt` runs
`open -t <prompt>`; both refuse a path that does not canonicalize inside
`~/.claude/routines`, and report the refusal in the row's second line.

The rows come from `routines::snapshot` (a cache refreshed on a thread at
most every 30 s, and only while this view is up; the panes list never shells
out): it parses `~/.claude/routines/routinectl list --json`, drops routines
marked `done`, and keeps the plist plus `launchctl` reader as
`read_from_disk`, the fallback when routinectl is missing or prints something
else than JSON, logged once. An empty list borrows the hint slot:
`reading routines…` until the first background read lands, `no routines
scheduled` after it.

Grouping and phrasing (`group`, `next_words`, `in_words`, `ago_words`) are
pure functions with the clock passed in, tested against
`tests/fixtures/routines/list.json` at the fixed clock the section tests
share (`2026-11-03T20:30`).

### 3.7.3 Asks list (`SidebarList::Asks`)

`src/window/sidebar_asks.rs` owns this list: the view model, the pure layout,
the drawing and the hit test. Two kinds of card share it. The
board (`ask::Board`) holds the questions, and `sidebar_ui::ask_cards` turns
them into `AskCardVm`s once per tick while this list is up, resolving each
`pane_id` to its tab across every window and checking
`claude_session::live_session_ids` for a conversation that outlived its pane.
The same read walks every pane and turns a `PromptPreview::Permission` into
a permission card, read from `Pane::permission_prompt`. The meta line reads
`N need you · newest first` while N > 0 and stays empty at 0; the open cards
are newest first, questions and permissions together.

A card (radius 8, `bg1` `TILE`, 1 pt `b3` `BORDER_SUBTLE`, 10 v / 12 h
padding, 8 pt apart):

| Part | Content | Style |
|---|---|---|
| Header (18 pt) | 8 pt `awaiting` dot, then `from` (the asker's name), the tab name, and the age at the right (`4m`, `2h`) | `Title` primary, tab `Summary` tertiary, age `Number`; `warn.text` from 2 h |
| Message | the question, wrapped, six lines then an ellipsis | `Secondary` (12) primary |
| Actions (28 pt) | OK 32 x 28 (check, `ok` colour), KO (cross, `crit`), Brief (pencil, primary), then at the right the ghost Open (only when the ask has a pane) and Moon, 26 wide | button ground `bg.hover` / `bg.pressed`; ghost icons tertiary, primary under the mouse |
| Note (14 pt) | a small green check and the first option, then a small red cross and the second 6 pt after the first label, so the pair sits together; right-aligned `Session ended` with an info icon when the ask has no pane and no live conversation | `Hint` (11) tertiary |

OK answers with the question's first action, KO with its second (or `No`
when it declared only one). The Board's answer path is the floating card's:
`AskQueue` on the app delegate is drained by the timer tick in `app.rs`, which
calls `Board::answer`, `Board::snooze` or `Board::unsnooze`. A click is
queued once: the card flips to `Sending to <from>...` (send icon) at once and
ignores further clicks; when the board drops the question the card reads
`Sent to <from>` (check-circle, `ok` colour) for 1.5 s before it disappears.
A sending click that the board never takes gives up after 3 s, so the card
is clickable again.

A decision card (`kova ask --decision <url>`, `AskCardVm::decision_url`)
carries a `Decision` tag in its header (the permission tag's chip), and its
action row reads Open (a text button on the `ACCENT` fill, `AsksHit::OpenDoc`),
OK and Brief: no KO. Open shows the sheet in the window's side panel
(`docs/side-panel.md`) and answers nothing; OK sends the first action, `Done`
by default. The note line says `Done once the sheet is filled in`.

The Moon opens a row of three small buttons under the actions: `1 h`,
`Tonight 20:00`, `Tomorrow 9:00`. Tonight is the next local 20:00 (tomorrow
when that one has passed), Tomorrow is the next local 09:00, and all three
keep the board's 24 h bound (`ask::SNOOZE_MAX_SECS`). The row grows the card
by 28 pt; on a narrow sidebar the three share the width equally and the
labels truncate.

The snoozed section is a folded 28 pt header at the bottom, `Snoozed N` with
a chevron (`AsksState::folded`, true by default, remembered per window). Open
it and each snoozed question is a dimmed card (alpha 0.55) on the same
geometry, without the action rows: `back at 20:00` on the left and a
`Show now` ghost button on the right, which cancels the snooze.

#### Permission cards

A pane whose screen asks permission is a card in the same list, newest first,
with no `kova ask` behind it. `sidebar_ui::ask_cards` builds it from the
pane's `PromptPreview::Permission` and the prompt it was read from
(`Pane::permission_prompt`), keyed `perm:<pane id>:<fingerprint>`:
`AskCardVm::from_permission`. It has the same frame, ground and padding, and:

| Part | Content | Style |
|---|---|---|
| Header (18 pt) | the dot, `from` (the pane's agent session name, else its title), a `Permission` tag, the tab, the age | `Title` primary, tag and tab `Hint`/`Summary` tertiary in a 1 pt `b3` chip, age `Number` |
| Message | the prompt's question (`Do you want to proceed?`), six lines then an ellipsis | `Secondary` primary |
| Code block | the command or file under a `bg0` ground, radius 4, 6 pt padding, Menlo 11, hard-wrapped at the font's advance, four lines then an ellipsis | 11 Menlo secondary |
| Actions (28 pt) | a green check (Approve), a red cross (Reject), a small `Always` chip when the prompt offers that option, and the ghost Open at the right | button ground `bg.hover` / `bg.pressed`; `Always` in `Chip` |

No Brief and no Moon: a permission has no instruction to add and nothing to
snooze. The buttons come from the option labels (`ParsedPrompt::choices`):
Approve is the first label starting with `Yes` that does not promise
`always`/`don't ask again`, Always the one that does, Reject the one starting
with `No`. A prompt whose options do not number from 1 (scrolled so option 1
is off screen) keeps its card, with only Open.

A click re-reads the pane's screen right then (`Pane::answer_permission`)
and compares `prompt_preview::permission_answer`'s fingerprint with the one
the card was built from. The hash covers the whole body block, every line of
a wrapped command or a diff (spacing normalized), not only its first line.
Equal: the option's digit alone goes to the pane's
PTY, the way Claude Code takes a numbered choice, and the card goes to
`Sending` like an asked answer. Different or gone: nothing is typed, the card
says `Prompt changed, open the pane` in `crit.text` for 5 s
(`AsksState::flash`, `FLASH_MS`), and the pane keeps the preview it just read,
so the list shows the new screen (or drops the card) on the next tick.

The list's `waiting` set (`sync_sidebar_asks_state`) holds the board's
questions and the permission cards of the last read, so a card leaves with
its prompt: Sending, then `Sent to <from>` for 1.5 s.

#### Brief overlay (the pencil)

`AsksHit::Brief` opens the Brief overlay on any question, including the ones
with no text field: an instruction typed on top of the ask. It is one card
over a 40 % black scrim covering the visible list; a click outside the card
cancels it. The card (`BriefLayout`) sits 12 pt from the top of the list and
12 pt from each side, 12 pt of padding, `bg1` ground, 1 pt `b3` border,
radius 8, top to bottom: the header (a warn dot, `Brief for <asker>` in
`Title`, the age in `Number` at the right), the question (`Secondary`),
the agent's options as chips, the instruction field, Cancel and Send, and a
two-line hint in `Hint` tertiary.

Chips are 22 pt tall, full radius, 1 pt `b3` border, `Chip` (11 Medium);
they wrap a row at a time. One is picked at most: a picked chip takes
`ACCENT` and white text (`TEXT_ON_FILL`), and a second click unpicks it.
The instruction rides on the answer whether or not a chip is picked.

The instruction field is native (`SidebarView` in `sidebar_view.rs`): one
`NSTextField` at most, owned by the view, keyed by the ask id, created and
focused when the overlay opens, moved only when its rect changes, removed
when the overlay closes or the question leaves the board. What was typed is
kept in `AsksState.drafts`, so reopening Brief on the same ask restores it.
Three lines (60 pt) at least, six (120 pt) at most, then the field scrolls;
placeholder `Your instruction`, Inter 12. Enter starts a new line, Cmd+Enter
sends (the view's `performKeyEquivalent`: AppKit routes the key equivalent
before the field editor, logged `sidebar: brief send (cmd+enter)`), Escape
cancels.
`KovaView::perform_key_equivalent` (`src/window/mod.rs`) answers NO while the
window's first responder is an `NSTextView`: the terminal's key map must not
take them first. Opening and closing the field is logged
(`sidebar: brief field open|close <id>`).

Send is disabled while no chip is picked and the field is empty. It queues
`AskAction::Answer`: the picked option with the text, or `ask::OTHER` with
the text alone. The text is trimmed, then capped at `ask::ANSWER_TEXT_MAX`
(8 KiB) on a character boundary. The card then goes to `Sending` like OK and
KO. `ask::accepts_external_answer` follows the same rule: text is allowed on
any button whatever the question's `input`, and `Other` is accepted with
text on any question.

A need row that lands on an ask (`ListHit::Need`, `Cmd+J`) switches to this
list, sets `AsksState::highlight` to the ask id and scrolls its card into
view once; the card's border takes `sel.rail` for two seconds
(`HIGHLIGHT_MS`, cleared on the tick). A permission row cannot name its card
before the list is read again (the id carries the screen's fingerprint), so
it sets `AsksState::highlight_pane` and `sync_sidebar` lights the card of
that pane on the same tick. With no card at all the list draws its own empty
state, not the hint line: a check-circle, `No open questions` and `Agents
ask here when they need you`, both tertiary and centred.

### 3.7.4 Agents list (`SidebarList::Agents`)

`src/window/sidebar_agents.rs` owns this list: the view state, the pure model
(`build`), the layout, the hit test and the drawing. The panes come from the
window's own facts; the roster comes from `crate::team`, which runs the
configured `[sidebar] roster_command` (nothing at all when it is absent),
cached and refreshed on a background thread every 60 s while the list is up,
never on the tick.

The meta row reads `N sessions in M tabs`.

#### Stat tiles

Four DS cards, a 2 x 2 grid below 360 pt of list width and four in a row from
360 up (`STAT_WIDE`): `bg1`, 1 pt `b3`, radius 8, 8 pt vertical and 10 pt
horizontal padding. The value is `Style::StatValue` (20 pt semibold); the
label is the 11 pt chip in `text.secondary`, after a 6 pt dot: `info` for
Working, `warn` for Need you (`waits_on_you`), tertiary for Idle, `crit` for
Error. A tile is a filter, and the selected one takes a `sel.rail` border.
The default is Working.

#### Sections

The selected filter's section comes first, then the shared sections, in this
order:

| Section | Holds | Note |
|---|---|---|
| `Working now` | `flags.working` or `flags.starting` | `longest first` |
| `Need you` | a pane with an open ask or permission, oldest first | |
| `Idle` | idle agents and bare shells, with their age | |
| `Error` | a turn that died on a provider error | |
| `By tab` | one row per Kova tab | four-dot legend |
| `Team` | the roster's managers | hidden without a roster |
| `Dedicated sessions` | the open ones, then `Closed N` | closed folded |

A section header is 24 pt: the label (11 medium) and its count in tertiary,
with the note right-aligned. An empty section draws one 22 pt `None` row. A
pane row is 40 pt, radius 6, `bg.hover` under the mouse: the state dot in a
16 pt box, the title (13 medium, primary), line 2 `<tab> · <detail>` (12,
tertiary), and the right meta, an age in `info` for working, the wait's age
in `warn` for need you, an age in tertiary for idle, `error` in `crit`. A
pane row focuses its pane; a need row opens the Asks list on its card
(`sidebar_open_need`).

A `By tab` row is 26 pt: the tab's 3 x 10 colour bar, its name, then four
22 pt right-aligned count columns (need you, working, error, idle), blank at
zero. Clicking it switches to Panes on that tab.

#### Team and reopening

A roster entry (`crate::team::Team`) matches a pane by its agent session
name, or a pane or tab title, case-insensitively. Its row shows the pane's
state dot when matched, a dimmed ring when closed, the name, the role in
tertiary (`· not open` when closed), and on the right the pane's meta or a
22 pt `Open` button, the Scheduled body's small button style: `bg.hover` at
rest, `btn2.hover` under the mouse and the press, an 11 pt Medium label.

The closed dedicated sessions fold under a `Closed N` header, folded at
start (`AgentsState::closed_folded`); every closed row has its own `Open`.

The roster comes from `[sidebar] roster_command`, an argv
(`["/usr/local/bin/team", "list", "--json"]`); absent or empty, both Team
sections are hidden and no process is spawned, and no path is baked into the
binary. `Open` runs the entry's `reopen` argv only when it is exactly
`team reopen <valid-name>` for that entry, with argv[0] replaced by the
configured roster command's argv[0] (same binary): anything else is refused
before a process starts. The run happens on a background thread, then the
roster refreshes. While it runs, the button says `Opening…` disabled: the
same box and ground, a tertiary label, no hit target at all. A failure says
`Could not open` in `crit` for 10 s (`ERROR_MS`), and the reopen is retried
after `OPEN_TIMEOUT_MS` (20 s) if the roster still shows the entry closed.

`tests/fixtures/team/list.json` holds the made-up roster the parsing and
matching tests use; no real name is tracked.

### 3.7.5 `Needs you` block (`SidebarList::Panes`)

The first thing in the Panes list, above the groups, scrolls with them, and
is left out entirely while `model.needs` is empty. A card on the tile ground
(radius 8, 1 pt `border.subtle`), 4 pt of padding, 6 under the last row and
10 before the first group. Its header (20 pt, `Style::Sort`) says `Needs you`
on the left and the counts at the right, right-aligned and cut at the card's
edge: `2 need you · 1 error · 1 failed`, the waiting count in `warn.text`,
errors and failed runs in `crit.text`, the ` · ` in tertiary.

One 28 pt row per need (`model.needs`, in `needs::need_items` order): a 16 pt
glyph box at 6 pt from the row's edge (8 pt `awaiting` dot for an ask or a
permission, `error` dot otherwise, the `x-circle` icon for a failed routine),
then the title (`Style::Title`, primary, tail-truncated): the asker's name
for an ask or a permission, the pane's title for an error, the routine's
name for a failed run; the detail (`Style::Secondary`, tertiary: the
question, the command a permission wants, the provider's error,
`Routine failed`) and the age at the right (`Style::Number`,
`warn.text` for asks and permissions, `crit.text` otherwise). The title keeps
at most half of the room so the detail always reads; below 280 pt of list
width the detail column is dropped and the title takes it
(`NEED_DETAIL_MIN_W`). Rows light to `bg.hover` under the mouse.

Folding (`sidebar_model::need_rows_shown`): every error and failed routine
always shows, then the asks and permissions fill up to four rows, two of them
at least. When anything would be folded away a 22 pt ghost button closes the
card: `Show all N` while folded, `Show less` once every row shows
(`SidebarState::needs_all`, in the model as `needs_all`).

Clicks: an error row focuses its pane (in any window), a failed routine opens
the Scheduled list on that routine (`open_routine`), an ask opens the Asks
list on its card and a permission opens it on its pane's card (2 s, the card
border in `sel.rail`; `AsksState::highlight` for an ask id,
`AsksState::highlight_pane` for a permission). A pane holding an open ask or
a permission prompt drops its meta column and says `waiting 4m · Answer` on
the subtitle instead: the age in `warn.text`, `Answer` an accent link whose
hit target (`ListHit::NeedAnswer`) lands where that pane's ask row would.

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
| Header chevron zone (16 pt)| toggle collapse, no tab switch                         | same               | header menu  |
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
| Sort toggle                | kova <-> activity (Panes list only)                     |                    |              |
| View switcher              | show that list (`sidebar_show_list`)                   |                    |              |
| Section header             | fold or unfold that section (`sidebar_sched_section`)  |                    |              |
| Routine row                | open it, or close it when it is open (`sidebar_toggle_routine`); a click also closes an open confirmation | | |
| Routine hover/body buttons | `Run now` / `Pause` / `Resume`: `routines::run_action` (background, queued result); `Open log` / `Edit prompt`: argv `open`, path-guarded; `Delete`: the inline confirmation | | |
| Inline confirmation        | filled `Delete`: `routinectl delete`; `Cancel`: close it |          |              |
| Escape over the list       | close the inline confirmation, like a lifted tile       |                    |              |
| `Needs you` row            | error: focus its pane (any window); failed routine: Scheduled, row open (`open_routine`); ask or permission: Asks, `highlight_ask` set | | |
| `Show all` / `Show less`   | toggle `needs_all`                                     |                    |              |
| Tile `Answer`              | same as that pane's ask row                            |                    | tile menu    |
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

`Tab order` or `Activity`: tabs ordered by the most urgent state of
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
order, minimized panes left out), then the other windows' in tab order.
`sidebar_model::next_pill` reads that count when no need item has a pane.
`Cmd+J` and the pill jump to `sidebar::next_unread(order, focused)`: the
first unread pane after the focused pane's place in that order, wrapping,
never the focused pane itself
(a status line says `Nothing to read` when nothing else is). IPC
`list-panes` reports the same rule as `unread`.

Before that order, `Cmd+J` walks what waits on the user
(`src/window/needs.rs`): errors oldest first, failed routines oldest first,
then permission prompts and open questions merged by age, with a pane's own
prompt and open question collapsed into one item. The app timer builds that
list once per tick (`KovaView::refresh_needs_shared`) and every window reads
the same one, as do the Asks badge (`waits_on_you`: permissions and asks
only, never errors or failed runs) and the Scheduled badge. A failed routine
has no pane to land on, so it switches the sidebar to Scheduled once, and
the next press falls through to the unread order above. `needs::NeedWalk`,
shared by the windows, lands on each need pane once per walk and then hands
the key to the unread order, so two failed turns cannot pin `Cmd+J` between
them; the walk starts over once it has visited them all or the list changes.

### 5.1 Where the question text comes from

`src/prompt_preview.rs` (pure) and `Pane::probe_prompt` (the trigger) read a
Claude pane's screen once its spinner stops (1 s debounce) and parse the
permission prompt with the daemon's grammar, numbered options included
(`ParsedPrompt::options`, `choices`, `fingerprint`). The parsed prompt sits on
the pane (`Pane::permission`) beside the preview the tile reads, both written
by the same read; `Pane::refresh_prompt_preview` is that read on demand, the
one a permission click goes through. Cleared on a rising edge of
`is_working()`, which is Claude taking its answer. The fallback reads the last
assistant text of the transcript (`~/.claude/projects/<slug>/<id>.jsonl`)
into a `TurnEnd { summary }`. Fixtures under `tests/fixtures/prompt/`.

## 6. Tokens (`sidebar::tokens`, RGB floats, the design system)

The palette comes from KovaLink `packages/protocol/tokens/ds.json`, the
design system, generated into `src/window/ds_tokens.rs`. `sidebar::tokens`
holds both generated sets as `Palette` values (`DARK`, `LIGHT`) with the
alpha channel dropped, and every call site reads the one in use through
`get()`. The set follows the terminal background (`[colors] background`, its
Rec. 709 luma strictly above 0.5 is light), picked at startup and again on
`reload-config` (`tokens::set_background`). `crit.solid` is `#ED3726` in
both sets.

```
ground            bg0
tile              bg1
tile.hover        hover
tile.pressed      btn2.hover
bg2               bg2
sel.row.bg        gray.1500 #2F2F2F dark / gray.100 #E9E9E9 light (spelled
                  out in `sidebar.rs`: a selected row's ground)
sel.row.text      gray.25 #FAFAFA dark / gray.1600 #242424 light
sel.row.rail      blue.400 #7498FF dark / blue.500 #4C74FF light
sel.bg            sel.bg (the icon switcher's selected box, never a row)
sel.rail          blue.400 dark / blue.500 light (stat cards, ask highlight)
border.subtle     b3   (= separator, hairlines)
border.strong     b2
accent            accent   (= unread, focus ring, links)
accent.pressed    accent.hover
text.primary      t1
text.secondary    t2
text.tertiary     t3
text.onFill       #FFFFFF
status.awaiting   warnDot
status.awaitingBg warnBg
status.working    infoDot
status.error      critDot
status.interrupt  crit
status.ok         okDot
status.warnText   warn
status.infoText   info
status.critText   crit
crit.solid        #ED3726 (spelled out: no dark token carries it), hover #CC2314
tabNone           t3
tab tint          TAB_COLORS[c] (Kova palette, mirrored by the phone)

top row 38 (switcher 4 x 26, gap 2, radius 6, right inset 10)  title 40 (name 14 in, pill right inset 12)
meta 20 (6 inside the list inset 12)  pill 24 (radius 12, content-sized, pad 9, icon 11, gap 4, badge 16, gap 6)
switcher badge 15 (radius 7.5, ring 2)  failure dot 8  sort ghost: icon 11 (gap 4, pad 4), height 16
list inset 12 (+6 top, +12 bottom)  group gap 16  header 28 (hover pill radius 14)
active card radius 8, pad 12, row gap 12  dot box 16
tile radius 10, pad 8/10  card radius 12, pad 10/12, bar 4
rows: title 20, secondary 16, question 16/line, gap 2  chip 18 (radius 9, pad 7)
hover button 24 (radius 7, gap 2, icon 16)  header + 24 (radius 8)
link icon 11 (gap 4)  actions row 20  hint 20  width 280, clamp 200..520
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
mode = "sidebar"              # "tabs" (default) | "sidebar"
sidebar_width = 280           # points, clamped to 200..520
sidebar_collapsed_default = false   # new tabs start folded when true

[sidebar]
# The roster command, as argv: the program, then the words that print the
# team roster as JSON. Absent or empty: the Agents view hides the Team and
# Dedicated sections and spawns nothing.
roster_command = ["/usr/local/bin/team", "list", "--json"]

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
