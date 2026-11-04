# Side panel

A web view down the right edge of the window, the mirror of the sidebar on
the left. It shows what a click in Kova points at that is not a terminal: a
decision sheet (an artifact), a website, a markdown or HTML file.

Code: `src/window/side_panel.rs` (the split, the header, the link rules and
the markdown page are pure and unit-tested; the AppKit view and the
`WKWebView` delegate are not), `KovaView::apply_layout` and `split_frames`
in `src/window/sidebar_ui.rs`.

## Opening it

| From | What |
|---|---|
| Cmd+click in a pane | an http(s) link, or a `.md` / `.markdown` / `.html` / `.htm` path (relative to the pane's directory, `~/`, absolute, `file://`, a `:line[:col]` suffix ignored). Cmd+hover underlines both. Cmd+Shift+click keeps the old way: the default browser. A link of another scheme still goes to the system |
| The ask card | Open on a `New decision` card (`kova ask --decision <url>`, or `--decisions <file.json>` for a native sheet) |
| The Asks view | Open on a card tagged `Decision` |
| IPC | `open-panel` with `target` (`docs/ipc.md`) |
| View > Side Panel, Cmd+Option+B (`toggle_side_panel`, `toggle-side-panel`) | hides it, or brings back the last page |

A library host's panes keep `url_at` (URLs only): path links are Kova.app's
(`TerminalState::link_at`).

## Layout

Header 38 pt (the sidebar's top row): the page title and, for a web page,
always its host (`Claude · claude.ai`, `header_title`), so a page cannot pass
for another site by its title alone; Inter 13 medium. Then
three ghost buttons at the right, 26 pt square, radius 6: back (disabled when
there is nothing to go back to), open in the default browser (the URL the
panel is on now, or the file), close. A double click on the header zooms the
window, a drag moves it. A 1 pt `b3` separator down the left edge, a 5 pt
resize zone over it.

Width: a share of the room right of the sidebar, 0.5 by default, 0.2..0.8,
at least 320 pt, the terminal keeping `splits.min_width`. When the three do
not fit, the sidebar yields first, then the panel. Dragging the edge sets the
share live; it is saved on mouse up as `panel_fraction` in
`~/.config/kova/prefs.json`. Closing keeps the page: the toggle brings it back.

The panel never squashes the panes: while it is open, each tab keeps the
width it has without it (`virtual_width_override` pinned, `panel_pin` in
`sidebar_ui.rs`) and scrolls horizontally; the focused pane is scrolled into
view. A width the user set themselves is left alone, and Kova's pins come off
when the panel closes.

## Web content

- WebKit's default data store, persistent: a login made in the panel stays
  across launches (`~/Library/WebKit/com.micktaiwan.kova`). Kova never types a
  password or approves a consent screen.
- The user agent ends with Safari's tokens, so sites treat the panel as
  Safari.
- A plain `target=_blank` link opens in the panel itself. A `window.open`
  (Sign in with Google, an OAuth consent) gets a real popup window built on
  the configuration WebKit hands over, so `window.opener` works and the login
  reaches the page; it closes itself at the end (`webViewDidClose:`) and
  shares the panel's data store.
- `mailto:`, `slack:` and other schemes go to the system only when the user
  clicked a link in the main frame; a page that redirects to one on its own is
  cancelled (logged). A clicked `file://` link to a markdown file is rendered
  rather than shown as raw text. Pages on `file:` (Kova's rendered markdown,
  local HTML) run no JavaScript.
- With the web view focused, its page owns the shortcuts (Cmd+W, Cmd+F, Cmd+R…);
  Cmd+A/C/V/X/Z are routed to it (Kova has no Edit menu). Only three Kova
  shortcuts stay Kova's there: the side panel toggle, the sidebar toggle and
  Cmd+J.

## Decision sheets

`kova ask --decisions <file.json>` (`src/decision_sheet.rs`, pure and tested):
Kova renders the decision-sheet JSON as a page in the design system's colours:
title, who asks, intro, links, one card per decision (options with the
recommendation tagged, Other, a note), a progress bar, Open only, "Take the
recommendation on the N left" (with a confirm), a general note, Done. Every
text of the sheet is escaped into the HTML by Rust.

It is the one page in the panel that runs a script, and the script only reads
the form and posts it (`window.webkit.messageHandlers.kovaSheet`). A post is
taken only from the panel's own web view, its main frame, on `about:blank` (the
page is loaded from a string with no base URL), while the panel shows a sheet;
the tick keeps it to what the sheet asks (`Sheet::answers_from`). Drafts are
saved on the question, on disk with it, so closing the panel or quitting Kova
loses nothing. Done answers the question with the summary and the panel shows
what was sent. A link in the sheet opens in the default browser: the sheet
stays the panel's page.

Open (on the floating card or in the Asks view) takes the decision off the
floating card, which moves on to the next question: it is answered on the
sheet, and stays in the sidebar's Asks until then. Not kept across a restart:
a relaunch puts it back on the card.

## Markdown

Rendered by Kova (`pulldown-cmark`: tables, task lists, strikethrough,
footnotes) into a page in the design system's colours (the palette the
terminal background picks, light or dark): Inter 14, `bg0` ground, code on
`bg2`. Raw HTML in the file is shown as text, never interpreted; a link keeps only an
http(s), mailto or relative target (no `javascript:`), an image only a local
one (a remote image would tell its server the file was opened), and the page
runs no script. The file is checked once a second while the
panel shows it and rendered again when it changes.
