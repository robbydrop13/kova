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

## What an id is about

An agent writes `CLA-10040` and the reader does not know what it is. Kova
says it: with `[links] refs_file` set to a JSON file of titles (`{"CLA-123":
{"title": "…", "url": "…" or null}, "#12345": {…}}`, written by an outside
tool, read again when it changes), an id of a text that the text does not
tell gets its title drawn after its first mention, in parentheses and
greyed (`.rt`): Kova's words, visibly not the author's. Once per id and per
text (a chat answer, a card's title, a line of En cours). The text itself,
and the transcript, are never changed. `src/refs.rs`.

| In the text | Drawn |
|---|---|
| An id the cache knows, with no subject in the text | `CLA-10040` then ` (its title)` greyed, 90 chars at most; the link goes to the cache's `url` when it has one |
| An id the text tells, at this mention or another: `CLA-1 (subject)`, `subject (CLA-1)`, `CLA-1 : subject`, `#13573, which keeps the screen on`, `\| #13908 \| subject \|`, after a markdown link to it, a run `CLA-1, CLA-2 et #3 (subject)` | nothing added |
| An id the cache does not know | nothing added; it links as before |
| An id in code, in a URL, in a heading | nothing added |
| `#1234`, `PR 13937` (not the five-digit `#12345`) | a link only when the cache knows it |

The forms that tell a subject are the ones the harness's form check
accepted (`claude-config/bin/formcheck`, `check_subject`), read on the id's
own line, with one difference: a comma followed by a conjunction (`#13900,
et son ticket …`) goes on with the sentence and tells nothing. In the chat,
the whole answer is one text (`chat::page::markdown`); a decision sheet and
the floating ask card do not draw titles.

`kova render chat|cockpit --refs <json>` draws them from a given file.

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
| Decision sheet, its `links` field | each link's `url`: an http(s) URL, or an absolute or `~/` path to a file the panel shows. `kova ask --decisions` refuses any other with the reason (missing, another type, relative) | `decision_sheet::link_ok`, `link_href` |
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
