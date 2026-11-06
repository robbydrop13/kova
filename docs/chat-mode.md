# The Cockpit's chat

The Cockpit shows a chief of staff's Claude conversation as a chat on its
left third, with an input box at the bottom and what waits on the user (its
`kova ask` questions, a permission prompt, a menu) as cards in the thread.
The chief's pane keeps running underneath as a terminal. Panes are
terminals only: there is no chat over a pane (removed 06/11/2026).

Code: the pure half in `src/chat/` (transcript reader, chat model, banner,
input gate, cards, page; all unit-tested), the AppKit half in
`src/window/chat_view.rs` (the web view on the column, not tested). Which
pane and where: `docs/cockpit.md`, Chat.

## Hiding and showing

| From | What |
|---|---|
| Cmd+Option+T (`toggle_chat`; not Cmd+Option+C, taken by Alfred's clipboard viewer), IPC `dispatch-action` `toggle-chat` | the column hidden or shown; outside the Cockpit, the Cockpit opens on its chat |
| The Cockpit's `Chat` button, the column's X | the same, from the page |

The column shows by default; the choice holds for the window's life. Until
the chief's Claude is resumed the banner says `Offline` and the thread shows
the transcript as it was.

## Layout

One `WKWebView` per window, created on first use, laid on the Cockpit's
left third under the column's top bar, above the Cockpit's own page. It
hides when the column or the Cockpit closes and while an overlay Kova draws
in Metal needs the window (the switcher, the search palette, the filter,
help, a rename); hidden, its state is dropped and the page comes back
fresh. Its data store is non-persistent.

The box takes the keyboard when the chat shows; the chief's pane is never
focused for it. With the box
focused, Cmd+A/C/V/X/Z edit its text; every other Kova shortcut stays
Kova's.

## The thread

Read from `~/.claude/projects/<slug>/<sessionId>.jsonl` (under the
session's `CLAUDE_CONFIG_DIR` when it has one; found in another project
folder when the session started elsewhere), the last 4 MB at open, then
only what is appended, as soon as it is: a kqueue on the file
(`src/chat/watch.rs`) is asked once per frame without waiting, so a line
reaches the chat one frame after Claude Code wrote it (a read every second
anyway, for a file replaced or not written yet). A line still missing its `\n` waits for
it; a file rewritten in place (`/clear`) is read again. On KovaLink's rules
(`link/daemon/src/transcript/jsonl.ts`): the lines of one request grouped by
`requestId` and ordered by `apiBlockIndex`, tool results joined to their
call by `tool_use_id`, a message sent mid-turn read from its
`queued_command` attachment, the harness's own `user` lines (`isMeta`, a
`<system-reminder>`…) never shown as the user's, sidechains and unknown line
types ignored. A large tool result Claude Code moved to a file is shown as
the transcript holds it, cut.

Readability, KovaLink's (`link/docs/13-chat-lisibilite.md`):

- the turn's state on the line above the box (`src/chat/banner.rs`): `Travaille · 1 min 12 s` (pulsing),
  `Attend ta réponse`, `Prêt · fini il y a 3 min`, `Hors ligne`, and the
  queue (`2 en attente, part à la fin du tour`). Driven by the session's
  events, never guessed: Claude Code's busy signal (title spinner or OSC
  7501 `working`, through the PTY), its
  hooks (`~/.claude/state/turns/<session id>`: `working` on
  UserPromptSubmit and `ready` on Stop from `claude-config/hooks/harness.py`,
  `waiting` on a permission prompt or a question from `notify.sh`; the folder
  is watched), an `AskUserQuestion` or `ExitPlanMode` call still without its
  result in the transcript, and the cards. A hook's `waiting` holds only
  while a dialog covers the `❯` line and the spinner is off; its `working`
  bridges the moment before the spinner shows (5 s at most, and not past a
  spinner that went off after it: Esc runs no Stop hook); its `ready` ends the
  turn even when Claude Code still reports `working` (unless it turned busy
  after the Stop), as it does while
  background agents run after its turn (`Waiting for N background agents to
  finish`, the prompt takes a message);
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
spaces only when the file exists. In an answer, a bare Linear id
(`CLA-123`, `CLAR-`, `CLAD-`) links to its issue and a bare `#12345` to that
pull request of the Claap repo, but not inside code, a link, a URL or a
heading. A click opens it in the side panel
(`side_panel::target_on_disk`, as `open-panel`), mailto in the mail app. Every message the page posts carries its load's nonce: one from an
earlier load is dropped. The page's content security policy runs
only the script carrying the load's nonce.

## The input box

Enter sends, Shift+Enter is a new line, at any time: the box never refuses
a message. The text goes to the pane's PTY as a paste (bracketed when
Claude Code asked for it), then Enter on its own 300 ms later, only when
Claude is the pane's foreground process and its turn is over (`Prêt`,
`chat::input::gate`). The Enter is written only while the prompt still
shows the pasted text and no permission prompt came up, and not at all when
it is more than 2 s late: the text then waits in the terminal's prompt, with
a note.

| The pane | The message |
|---|---|
| `Prêt`, the `❯` line empty | sent |
| `Prêt`, the `❯` line holds text (a draft typed in the terminal, Claude Code's grey suggestion) | sent, pasted after it: both leave as one message |
| `Travaille`, `Attend ta réponse`, no `❯` line (a menu, a dialog), no Claude | queued |

The queue shows at the end of the thread as `En attente` bubbles, in order,
each with Modifier (edited in place, Enter saves) and Annuler (its text goes
back to the box) until it leaves. It leaves one message per turn, the first
once the turn is over, the next at least 1.5 s after an Enter (the turn it
started has to show first). A message sent while others wait goes behind
them.

Cmd+V or Ctrl+V with an image on the pasteboard writes it where a terminal
paste does (`/tmp/kova-paste-<ms>.png`, `appkit_input::pasted_image_path`)
and shows it as a thumbnail above the box (× removes it); its path goes
before the text of the next message, as the terminal hands an image to
Claude Code. Only a file Kova wrote for this page's paste goes along
(`chat::input::with_images`). Text pastes as usual.

Control characters are removed (tab and newline
kept), 32 KB at most (a 20 minute dictation). The empty prompt is read off the screen like KovaLink's
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
Listening stops by itself at 20 minutes and sends.

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
endpoint (`dictation.eu.assemblyai.com/v1/transcribe`, the recorded-audio endpoint, 20 minutes at most, multipart:
Claap's config, then the raw 16 kHz mono PCM), with the cleanup rules as
`llm_instruction` (`~/.config/kova/voice-instruction.txt` when it exists,
where Claap's own rules live on this Mac only since this repo is public,
else a short generic set in `voice.rs`), `rewrite_mode` `fast`,
`language_codes` `["fr","en"]` and `[chat] voice_vocabulary` (optional
list of names and jargon, default empty) as `keyterms_prompt`. The answer's
`llm_response` goes in, or its verbatim `text` when the cleanup failed.
429 and gateway errors are retried twice, like Claap's back. The wait for
the answer grows with the audio: 30 s, plus 10 s per minute. The key is
read from the Keychain at each dictation (`security find-generic-password
-s assemblyai-api-key -w`) and only ever reaches the request's
`Authorization` header; nothing writes or logs it. Less than 300 ms of
audio sends nothing. The microphone is an AudioQueue input; the first use
shows macOS's prompt with `NSMicrophoneUsageDescription` (Info.plist).
The chat going out of sight cancels listening and a text on its way. A press long after a hold began (its release
lost) ends the hold.

## Cards

At the bottom of the thread, above the box:

- the pane's questions on the board (`kova ask` from this pane and this
  conversation, not snoozed): their buttons, a field when they take text;
  the answer goes through the board's queue like an Asks card's, checked
  against the question's buttons (`ask::accepts_external_answer`);
- a decision sheet: Ouvrir la fiche, which shows it in the side panel (the
  floating card's path);
- a handoff: its step (Ouvrir, Lancer) and Fait, the step performed by the
  board without answering;
- every question is drawn as a Cockpit card (`cockpit::card_html_with`,
  `cockpit::CARD_CSS` with the design system's tokens, light or dark, set on
  `#asks`): its first line as the title, what it wants, who asked and when,
  the rest of the message and its answers under it. Its three icons are the
  Cockpit's: the clock opens the same snooze menu (the question comes back
  at that date, `Board::snooze`), X and the check take it off the Asks
  (`Board::close`, logged with reason `dismissed` or `done`; its `kova ask`
  exits 3 as on a withdrawal). When an attention entry stands for a sheet
  (its action `kova-sheet:<id>`), the card is that entry's opened Cockpit
  card, the entry gets the same verb (`attn snooze`, `attn close --reason
  dismissed|done`) and the card has the instruction field (`attn act ID
  instruct --text=...`, Enter sends, Shift+Enter is a new line);
- the menu Claude Code waits on, read off the pane's screen
  (`prompt_preview::parse_menu`, one grammar for a permission prompt, an
  AskUserQuestion and the plan approval, fixtures in `tests/fixtures/prompt/`;
  a label wrapped in a narrow pane and a named session's bottom rule
  included): each option a button, a field, the plan from the transcript
  (`ExitPlanMode`'s `plan`). A click types the option's digit only while that
  menu is still on screen; the option that takes text (`Type something.`,
  `Tell Claude what to change`) gets the digit, the text, then Enter, 250 ms
  apart (written together, Claude Code reads them as one key), Enter only
  once the option shows the text. Text with another option follows as the
  next message. While such a menu waits, the box answers it at once. A wait
  Kova cannot read shows a card saying so, never guessed options;
- the permission prompt read on the pane's screen, when no menu was read: Yes, the option that
  always allows, No; the digit is typed only while the screen still shows
  that prompt (`Pane::answer_permission`).

## Not done yet

- Other agents (omp, Codex): terminal only.
- The page's colours follow the terminal's palette when it is loaded, not a
  switch made while it is shown.
- The expanded state of tool rows is lost when the page reloads (the
  column closed, or the other chief picked).
- The queue lives with the page: messages still queued are dropped when
  the column closes or the other chief is picked.
- The first 4 MB are parsed on the main thread, and every change renders the
  shown entries' markdown again.
