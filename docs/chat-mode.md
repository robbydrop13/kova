# Chat mode

A pane running Claude Code can show its conversation as a chat instead of
its terminal grid, with an input box at the bottom and what waits on the
user (its `kova ask` questions, a permission prompt) as cards in the thread.
The terminal keeps running underneath: toggling back shows it as it is.

Code: the pure half in `src/chat/` (transcript reader, chat model, banner,
input gate, cards, page; all unit-tested), the AppKit half in
`src/window/chat_view.rs` (the web view over the pane, not tested).

## Toggling

| From | What |
|---|---|
| Cmd+Option+T (`toggle_chat`; not Cmd+Option+C, taken by Alfred's clipboard viewer), Pane > Show as Chat, IPC `dispatch-action` `toggle-chat` | the focused pane as a chat, or its terminal again |
| The banner's Terminal button | the terminal again |

A pane without a Claude conversation stays a terminal, with a note in the
status bar. One chat pane per tab: turning one on turns the others of its
tab off. The choice is saved with the session (`chat` on the saved pane), so
a restored pane comes back as a chat; until its Claude is resumed the banner
says `Offline` and the thread shows the transcript as it was.

## Layout

One `WKWebView` per window, created on first use, laid over the active
tab's chat pane, the pane's status bar left visible. It hides when the tab
has no chat pane, when the pane is minimized or too small, and while an
overlay Kova draws in Metal needs the window (the switcher, the search
palette, the filter, help, a rename). Its data store is non-persistent.

The keyboard follows the focus: a click into the chat focuses its pane, the
chat pane focused gives the box the keyboard, another pane focused takes it
back. With the box focused, Cmd+A/C/V/X/Z edit its text; every other Kova
shortcut stays Kova's.

## The thread

Read from `~/.claude/projects/<slug>/<sessionId>.jsonl` (under the
session's `CLAUDE_CONFIG_DIR` when it has one; found in another project
folder when the session started elsewhere), the last 4 MB at open, then
only what is appended, every 200 ms. A line still missing its `\n` waits for
it; a file rewritten in place (`/clear`) is read again. On KovaLink's rules
(`link/daemon/src/transcript/jsonl.ts`): the lines of one request grouped by
`requestId` and ordered by `apiBlockIndex`, tool results joined to their
call by `tool_use_id`, a message sent mid-turn read from its
`queued_command` attachment, the harness's own `user` lines (`isMeta`, a
`<system-reminder>`…) never shown as the user's, sidechains and unknown line
types ignored. A large tool result Claude Code moved to a file is shown as
the transcript holds it, cut.

Readability, KovaLink's (`link/docs/13-chat-lisibilite.md`):

- a banner at the top: `Working · 1 min 12 s` (pulsing), `Waiting for you`
  (a card waits), `Done 3 min ago`, `Offline` (no Claude in the pane);
- the assistant's text full width, rendered markdown, no bubble; the user's
  message in a quiet bubble on the right;
- only the user's messages and the assistant's final text (no tool call
  after it in its turn) show. Everything between two of them folds into
  one `N actions en coulisse` line, a click unfolds it, compact: tool calls
  as one-line rows (`Read  …/src/boot.ts`, `Run  npm test`), state at the
  right (running, ✓, ✕), a click unfolds the result, a run past five folded
  again into `N actions`; the text that went with the calls; notes
  (`Interrupted`, a slash command); another session's message
  (`Message de <name>`) and a finished background task, never shown as the
  user's (a peer's message is queued as a `prompt` too: its origin decides).
  System reminders are dropped. What was unfolded stays unfolded while the
  page lives. The cards (questions, permission prompt) are never folded;
- while Claude works with no call running, a `Réflexion` line ends the
  thread, a light sweeping across it (still under `prefers-reduced-motion`);
  a click unfolds the reasoning when the transcript holds it (it often keeps
  only its signature);
- the last 20 exchanges at open, `Show earlier` above for more; the view
  opens on the last user message; nothing folds by itself.

Every text from the transcript is escaped by Rust. Markdown shows raw HTML
as text, keeps a link only to http(s), mailto or a local `.md` / `.html`
file (a link to anything else is shown as its text) and never loads an
image. A bare http(s) URL, or an absolute or `~/` path to a `.md` / `.html`
file, is a link too (`src/chat/links.rs`), but not inside code; a path with
spaces only when the file exists. A click opens it in the side panel
(`side_panel::target_on_disk`, as `open-panel`), mailto in the mail app. Every message the page posts carries its load's nonce: one from an
earlier load (another pane) is dropped. The page's content security policy runs
only the script carrying the load's nonce.

## The input box

Enter sends, Shift+Enter is a new line. The text goes to the pane's PTY as
a paste (bracketed when Claude Code asked for it), then Enter on its own
300 ms later, only when Claude is the pane's foreground process, idle, with
an empty prompt (`chat::input::gate`). The Enter is written only while the
prompt still shows the pasted text and no permission prompt came up, and
not at all when it is more than 2 s late: the text then waits in the
terminal's prompt, with a note.

| The pane | The message |
|---|---|
| idle, the `❯` line empty | sent |
| working | queued, sent once idle (`Queued: …`, Cancel) |
| a permission prompt on screen | queued |
| no `❯` line in the first column (a menu, a dialog) | queued |
| the `❯` line holds text (the user's draft in the terminal) | refused, back in the box |
| no Claude | refused, back in the box |

Cmd+V or Ctrl+V with an image on the pasteboard writes it where a terminal
paste does (`/tmp/kova-paste-<ms>.png`, `appkit_input::pasted_image_path`)
and shows it as a thumbnail above the box (× removes it); its path goes
before the text of the next message, as the terminal hands an image to
Claude Code. Only a file Kova wrote for this page's paste goes along
(`chat::input::with_images`). Text pastes as usual.

One message at a time. Control characters are removed (tab and newline
kept), 8 KB at most. The empty prompt is read off the screen like KovaLink's
daemon does (`composerIsEmpty`); Claude Code's start hint (`Try "…"`) counts
as empty.

## Dictation

Voice typing as Claap's desktop app does it (`src/voice.rs` pure and
tested, `src/voice_io.rs` the native half; how Claap does it:
`docs/voice-typing.md`). Hold Cmd+Shift+M (`toggle_voice`, IPC
`dispatch-action` `toggle-voice`) or the mic button and talk, release to
insert (push-to-talk); a tap under 300 ms keeps listening hands-free until
the next press or the pill's check. Esc anywhere in Kova cancels, while
listening or while the text is on its way; a press meanwhile is ignored.
Listening stops by itself at 120 s and sends.

A pill above the box shows the state: green bars following the voice
(hold), the same with cancel and finish buttons (hands-free), a moving wave
(transcribing), then a short line on a miss or a fault (`Rien entendu`,
`Annulé`, `La dictée a échoué`, `Limite de dictée atteinte`,
`Impossible de joindre AssemblyAI`, `AssemblyAI a refusé la clé`,
`Micro indisponible`, `Clé AssemblyAI absente du Trousseau`). Sounds:
Tink at the start, Pop at the end, Basso on a fault (macOS system sounds).
The clean text goes in at the cursor, spaced from what is around it. The
clock button lists the last 10 dictations (memory only, for the app's
life); a click puts one back at the cursor. With `[chat] voice_auto_send =
true` the box sends as soon as the text is in.

The audio stays in memory and goes in one request to AssemblyAI's dictation
endpoint (`dictation.eu.assemblyai.com/v1/transcribe/live`, multipart:
Claap's config, then the raw 16 kHz mono PCM), with the cleanup rules as
`llm_instruction` (`~/.config/kova/voice-instruction.txt` when it exists,
where Claap's own rules live on this Mac only since this repo is public,
else a short generic set in `voice.rs`), `rewrite_mode` `fast`,
`language_codes` `["fr","en"]` and `[chat] voice_vocabulary` (optional
list of names and jargon, default empty) as `keyterms_prompt`. The answer's
`llm_response` goes in, or its verbatim `text` when the cleanup failed.
429 and gateway errors are retried twice, like Claap's back. The key is
read from the Keychain at each dictation (`security find-generic-password
-s assemblyai-api-key -w`) and only ever reaches the request's
`Authorization` header; nothing writes or logs it. Less than 300 ms of
audio sends nothing. The microphone is an AudioQueue input; the first use
shows macOS's prompt with `NSMicrophoneUsageDescription` (Info.plist).
The chat going out of sight cancels listening; a text already on its way
lands when the chat is back. A press long after a hold began (its release
lost) ends the hold.

## Cards

At the bottom of the thread, above the box:

- the pane's questions on the board (`kova ask` from this pane and this
  conversation, not snoozed): their buttons, a field when they take text;
  the answer goes through the board's queue like an Asks card's, checked
  against the question's buttons (`ask::accepts_external_answer`);
- a decision sheet: Open, which shows it in the side panel (the floating
  card's path);
- a handoff: its step (Ouvrir, Lancer) and Fait, the step performed by the
  board without answering;
- the permission prompt read on the pane's screen: Yes, the option that
  always allows, No; the digit is typed only while the screen still shows
  that prompt (`Pane::answer_permission`).

## Not done yet

- Other agents (omp, Codex): terminal only.
- The page's colours follow the terminal's palette when it is loaded, not a
  switch made while it is shown.
- The expanded state of tool rows is lost when the page reloads (another
  pane shown as a chat in the window).
- The outbox lives with the page: a message still queued is dropped when
  the window shows another pane as a chat.
- The first 4 MB are parsed on the main thread, and every change renders the
  shown entries' markdown again.
