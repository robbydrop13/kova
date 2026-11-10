# Links: one module for every page

Rule: no path to a local file the user cannot open in the side panel,
anywhere Kova shows a text as a page. `src/links.rs` finds the links, draws
them and opens them; the pages only call it.

## What is a link

| In the text | Link when | Opens |
|---|---|---|
| `https://…`, `http://…` | always | the page, in the side panel |
| An absolute or `~/` path to a `.md` / `.html` file | as written | the file, in the side panel |
| Any other path to a file the panel shows (`side_panel::FILE_EXTENSIONS`: markdown, HTML, png, jpg, gif, webp, svg, pdf, txt, log, json): another extension, a path with spaces, a relative path | the file exists | the file |
| `[label](destination)` to one of the above | as its destination | the destination, named by the label |
| `CLA-123`, `CLAR-123`, `CLAD-123` standing alone (`linkify_refs`) | always | its Linear issue |
| `#12345` standing alone (`linkify_refs`) | always | that pull request of the Claap repo |

Existence is the opener's own test (`side_panel::target_on_disk`), so a link
drawn is a link that opens. A relative path is read against a base folder
and its parents up to `$HOME`: the pane's folder in the chat
(`links::set_base`), none elsewhere (a relative path stays text there). A
path to a file of another type (`.rs`, `.zip`), an API route
(`/v1/list.json` with no such file), a bare file name stay text.

## The functions

- `find` / `find_with_refs`: the links of a text, in order.
- `linkify` / `linkify_refs`: the text as HTML, escaped, each link an
  anchor (`<a href="…">` for the web, `<a href="#" class="path"
  data-path="…">` for a file). Each page's script posts a click on an `<a>`
  to Kova (`link` in the chat, `open` in the Cockpit), which calls `open`.
- `open(href, cwd)`: what the side panel shows for a clicked link. The one
  opener.
- `pieces`: for a surface that is not that HTML (a decision sheet in the
  side panel, where a link is a navigation, and the floating `kova ask`
  card, an attributed string): the text cut into plain pieces and links,
  a file as a `file:` URL (`file_url`, read back by `path_of_file_url`).
- `targets`, `label`: what a text's links open, and what a lone link reads
  as (a file's name, a URL without its scheme).

## Where

| Surface | Texts linked | Code |
|---|---|---|
| Chat | the assistant's text (markdown, a whole code span included), the user's bubble, tool results | `chat/page.rs` |
| Cockpit card | title, needs, body, detail, a question's message, the instruction sent, what a source says around its path | `cockpit.rs` (`linked`) |
| Cockpit card, sources and action | each source's first link and the action, as buttons named by `label` | `cockpit::source_target`, `entry_detail` |
| En cours, Fait | a click's title, instruction and result, an agent's line and its first instruction | `inflight.rs` |
| Catch-up band | a project's headline, a change, a waiting item's title and needs | `catchup.rs` |
| Decision sheet, long question | every text (`rich`, `inline`), a whole code span included; a file opens in the panel, Back returns to the sheet | `decision_sheet.rs`, `side_panel.rs` (navigation policy) |
| Floating `kova ask` card | its message | `ask_card.rs` |

Not linked: an editable field (a draft, a note), a notification, a sidebar
tile, a text cut before the end of a path (`…`), a code block in the chat.
The terminal has its own Cmd+click (`TerminalState::link_or_url_at`), and a
markdown file shown in the panel its relative links.

`text_links.rs` still cuts an ask's text into URLs of five schemes
(`slack://`, `linear://`, `notion://` open their app) and markdown links;
`links::pieces` then links what it leaves as text.

## Seeing it

`kova render` (`docs/render.md`) writes each page; a before and an after of
the same data show what a change links.
