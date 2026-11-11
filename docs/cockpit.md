# Cockpit

Every action that waits on the user, on one page laid over the panes. Decided
by Robin on 05/11/2026: it replaces the sidebar's `Needs you` block.

## Entry

The first row of the Panes list (`sidebar-spec.md` § 3.7.5): `Cockpit`, the
count of what waits on the user (`needs::waits_on_you`, unchanged), a red dot
when an action is urgent. A click opens the page, a second click or Escape
closes it. Cmd+J and the Next pill are unchanged; Cmd+J and a click on a tile
close the page, since they land on a pane it covers.

## Page

`src/cockpit.rs` (pure, tested) renders it; `src/window/cockpit_view.rs` hosts
it: one `WKWebView` per window over the active tab's panes area
(`panes_viewport_for_tab`), hidden while a Metal overlay is up, like the chat.

- Items: attention entries of level `now` and `point` (not `memo`), read from
  `[attention] command` (`attn list --open --json --scope all`), plus the live
  needs (`needs::need_items`): permission prompts, open `kova ask` questions
  and decision sheets, turn errors, failed routines. The `now` attention need
  is not listed twice: the queue's own entry stands for it.
- Priority: the entry's `prio` (1..3), else `now` 2 and `point` 3; permission
  prompts and errors 1; asks, sheets and failed routines 2. Sections Urgent,
  Aujourd'hui, Cette semaine; oldest first inside a section.
- Scope: the entry's `scope` (`pro` shows as Claap, `perso` as Perso). A live
  need takes it from its cwd: the routine's `cwd`, the pane's cwd for a
  permission or an error. Under `[attention] perso_dir` (`~/` expanded) it is
  Perso, else Claap; an ask, or no `perso_dir`, is Claap. The filter `Tout · Claap · Perso` is
  kept in the window's state.
- Buttons (Robin, 06/11/2026), one rule for every card, in the Cockpit and
  in the chat's cards (`cockpit::primary`, `cockpit::buttons`): the icons
  top right; at most one blue button, named by what the user does, never
  `Ouvrir`; every other action a plain text link left of it.

  | Card | Blue button | What it does | Links |
  |---|---|---|---|
  | `would`, `review`, `decision` with a prepared action | `Valider` | `act_command ID Valider` | `Voir`, `Refuser` |
  | Draft | `Valider` (`Valider la correction` once rewritten) | `act_command ID Valider` / `Corriger --text=` | `Corriger` (opens the consigne field; `Annuler` once rewritten) |
  | Entry naming a waiting ask, decision sheet | `Décider` | opens the sheet in the side panel | |
  | `decision` with nothing prepared | `Décider` | opens the card on its `Consigne` | `Voir` |
  | `test` | `Tester` | opens the card (the plan) on its `Consigne` | `Voir` |
  | `blocked`, `ask` (and a future `brief`) | `Brief` | opens the card on its `Consigne` | `Voir` |
  | `info`, `auto`, any other kind | none | the check icon is the action | `Voir` |
  | Permission | `Autoriser` | answers the prompt | `Refuser` |
  | Question | `Répondre` | opens the card on its answer | |
  | Turn error | `Voir le pane` | closes the page, focuses the pane | |
  | Failed routine | `Voir le log` | the Scheduled tile's log | in the opened card: `Lancer maintenant`, `Mettre en pause`, `Modifier le prompt` |

  `Voir` shows the entry's `action` in the side panel
  (`side_panel::open_target`). No kind means "a brief from Robin" in the
  queue: `blocked` (an agent stuck on him) and `ask` (he has something to
  give) are the two that wait on his words. A decision sheet
  reads `Décider` rather than `Ouvrir la fiche`: the verb is what he does, the
  field is where. Gone a go, the card keeps its links only and says how far the click went (below). A
  question opened in the chat shows its answers as its buttons. `Refuser` /
  `Autoriser` answer a permission prompt like the chat does, only if the
  screen still shows the prompt the card was rendered from.
- Waiting on a go (Robin, 06/11/2026): `Refuser` opens a small field
  `Pourquoi ? (facultatif)` under the card: Enter runs `close_command ID
  --reason dismissed`, plus `--note=<reason>` when one was typed (one argv
  element), and the card leaves; Escape folds the field (and only it). No
  close command, no `Refuser`; no act command, no `Valider`. `Valider` runs
  `act_command --exit-codes ID Valider` (the chief of the scope receives
  it). A click on an agent's card (`Valider`, a correction, an instruction)
  ends one of three ways, read from the command's exit code
  (`attention::delivery_of`): 0, `Transmis à l'agent` (green; an
  instruction: `Consigne remise à l'agent`); 3, recorded but not handed
  over yet, `Noté, pas encore remis à l'agent` (amber; the click is not
  offered again); any other code, `Non transmis : <first line of stderr>`
  (red, a minute; an instruction's stays until the next one), the card in
  place with its buttons. A click refused before the command ran (the entry
  gone from the queue, no command) says so the same way.
- The tasks and their five states (11/11/2026). When `inflight_command`
  gives `tasks` (the queue's own list, already sorted: the open ones
  whatever their age, the done and dropped ones of the last 24 h), `En
  cours` shows them first, in five sections in the queue's order, each with
  its state in words and its count, an empty one included: `À faire`,
  `Attend Robin`, `Attend un autre`, then `Fait (24 h)` and `Abandonné
  (24 h)`, folded and one click away (a section the user opened stays open
  across refreshes). A row is the task's title, the session that has to act,
  the state's word again, and what the state says: the next action, who and
  what it waits on, the proof (a link when it is one), why it was dropped
  (`dismissed`, `nogo`, `superseded`, `retiree`, `ttl`, `drop` in words, then
  its `why`). Kova filters nothing by age. The view's count is the open
  tasks. The clicks and the agents follow, as before. Without `tasks` (an
  older queue, or its legacy reading) the list is what it was
  (`inflight::tasks_html`, `list_html`).
- A go, its proof, its failure (10/11/2026). The queue says what became of a
  click (`inflight_command`: `go`, `done` only with a proof, `result`, and
  `outcome`: `proof`, `superseded`, `failed`, `no_proof`, `done` or
  `dismissed`), and `En cours` and `Fait` show it (`inflight::fate`, which
  reads `outcome` and falls back on the words of `result` when the queue
  does not give it). `superseded`: the action became pointless; `Sans objet`
  and its reason in grey, kept in `Fait` but never shown as `Fait` nor as a
  proof. The others: `preuve : X`, `Preuve :`
  and X as a link when it is one (`links`), in `Fait`; `échec : why`, `Échec`
  in red, still in `En cours`; `carte fermée sans preuve`, `Sans preuve` in
  amber, still in `En cours` (nothing proves the action took place). An
  instruction that is a go reads `Go`. A card back in the list after a
  failure (`failed` in the queue's JSON) says `Échec : why` at its head, with
  its buttons; one that carries a `proof` shows it under its needs.
- Where a draft really goes (10/11/2026): when the draft carries a
  `target` (what the sender's tool takes: a Slack id, a `#channel`, `/<ts>`
  for a thread), it shows in small type next to the `to` name, so the user
  sees the destination and not only a name. No `target`, nothing more. When
  `act_command` sends a validated draft itself and says so on its first
  line (`envoi par le code (slack à U123)`, `attention::CODE_SEND`), the
  card shows that line as it is, where it would say `Transmis à l'agent`.
- Whose click it is (10/11/2026). The queue's CLI takes a click for the
  user's when the process that runs it carries the click key. With
  `[attention] click_key_file` set, Kova reads that file at each click (one
  line; never kept, never logged, never written by Kova) and hands the key
  to `act_command`, `close_command` and the snooze command in their
  environment (`attention::CLICK_KEY_ENV`, and `CLICK_FROM_ENV` = `kova`),
  never as an argument, which any process can read. Only a click on the
  Cockpit's or the chat's page runs these commands (`attention::perform_with`):
  no IPC command, no `kova` subcommand, no `kova render` does, and a test
  holds that (`only_a_click_on_a_page_can_carry_the_key`). No file, or one
  that does not read: the click goes without a key, as before. Who clicked
  (`--by <name>`) is given in `act_command` and `close_command` themselves.
- The draft the user saw (10/11/2026). The queue gives each draft a
  fingerprint (`draft.seen`); a `Valider` or a correction sends back the one
  of the version the page shows (`--seen=<fingerprint>`,
  `attention::with_seen`). When the draft changed since, the command fails
  and the card says so (`Non transmis : …`), then shows the new draft at the
  next read. A second click on a version already sent answers `déjà parti
  …`, shown as it is, not as a failure. The target reads in words when the
  queue gives them (`draft.target_label`, the raw id in the tooltip; amber
  for `target_kind` `external`). A card back with `uncertain` (nobody knows
  whether the message left) says `Peut-être parti : <what to check>` at its
  head, the queue's own words, and offers two buttons and no other on its
  draft: `C'est parti` (`act_command ID parti`: the user saw the message;
  the card closes) and `Renvoyer` (`act_command ID Renvoyer`: nothing left,
  a new go), both with the click key and `--seen` like any go, neither in
  blue. In `En cours` such a click reads `Peut-être parti`, one being sent
  `Envoi en cours`. A click that changed nothing because this version is
  already taken exits 0 with a first line that says so (`déjà envoyé par le
  code…`, `envoi par le code … incertain`, `envoi par le code en cours…`,
  `déjà remis à…`, `attention::ALREADY`): the card shows that line as it
  is, in a neutral tone, neither `Transmis à l'agent` nor a failure.
- `No go` (10/11/2026), on an action plan only: a `decision` posted by
  `veille-actions` with its plan in the body (`attention::PLAN_FROM`). It
  runs `close_command ID --reason nogo`: the plan's agent learns a no go,
  where the x says the card was not relevant. The card leaves at once.
- The day's journal: with `[attention] journal_file` (a path with
  `{date}`), a quiet `Journal du jour` link under the list opens today's
  file in the side panel, when it exists. Routines write there what they
  did instead of posting cards.
- Icons (every attention card, top right, Feather, with a tooltip): clock
  `Plus tard`, x `Pas pertinent, retirer` (`close_command ID --reason
  dismissed`), check `C'est fait` (`close_command ID --reason done`). The clock
  opens a small menu in the page: `Dans 1 h`, `Ce soir` (19:00, or tomorrow
  19:00 once past), `Demain matin` (09:00), `Lundi prochain` (09:00),
  `Choisir…` (a date and time field); it runs `snooze_command ID
  --until=<ISO 8601 with offset>` (default: `close_command`'s program with
  `snooze`). The card leaves the page at once and stays hidden until the queue
  drops it (60 s at most); a failed command brings it back with its words.
  A waiting `kova ask` (sheet or question) has the same three icons (Robin,
  09/11/2026), acted on the board itself: the clock snoozes it, the x closes it
  `dismissed` (its `kova ask` exits 3 with `ask::DISMISSED`, and the same
  question from the same asker is refused until Kova restarts), the check
  closes it `done` (`ask::DONE`). The other live needs keep their own buttons,
  no icons.
- `Voir plus`: the list shows the first 10 cards in the priority order, then
  `Voir plus (N)` (N left) adds 10 more. Section and filter counts cover the
  whole list. Opening the page or changing the filter starts again at 10.
- Screenshots (Robin, 09/11/2026): an entry's `before` and `after` (absolute
  paths, `attn add --before --after`) show in the card itself, labelled Avant
  and Après, side by side when the card is wide enough (300 px each at least),
  embedded as `data:` URIs (6 MB each at most, read once per file version; the
  page's policy allows `img-src data:`). A click shows one full size over the
  page; a click or Escape closes it. A file that cannot be read says why.
- Drafts: an entry with a `draft` shows it in an editable field under
  `À <to>  <channel>  Objet <subject>`. `Valider` runs `act_command ID
  Valider`; once the text is rewritten it reads `Valider la correction` and
  runs `act_command ID Corriger --text=<text>`, the text one argv element
  (`attention::clean_text`: no control character but tab and newline, 16 KiB
  at most). `Corriger` opens the card on its instruction field (Robin,
  07/11/2026: a consigne to the card's agent, `act_command ID instruct`, not an
  edit of the text); once the text is rewritten it reads `Annuler` and puts
  the draft back. Kova
  never sends anything: the chief who receives the click does.
- Selecting and copying (Robin, 06/11/2026): every text of the page selects
  and copies, the Cockpit's web view gets Cmd+A/C/V/X/Z like the chat's
  (`performKeyEquivalent`, Kova has no Edit menu; before, Cmd+C copied the
  terminal's selection and Cmd+V typed into the pane). A push that would
  replace a part (list, head, chat column) holding a selection waits until the
  selection goes or the page loses the keyboard; a `Voir la carte` lets the
  list in first. A click
  that ends a drag-select neither opens nor folds the card.

Every text is escaped into the HTML in Rust. The page's script (CSP nonce)
only posts clicks, the filter and the edited text (`kovaCockpit`); messages
from another frame, another URL or an earlier load are dropped. The tick
gathers the items every 250 ms and pushes only the parts that changed; a draft
being rewritten keeps its text, caret and focus through a push.

## Filtres

Filters work like Linear's (Robin, 10/11/2026; `cockpit::filters_html`). The bar under the title holds one `Filtrer` control (funnel icon, styled like `Par priorité` on the right). It opens a menu of every dimension with its values and their counts, a check on the chosen one: `Espace` (Claap, Perso, the scope) and `Vue` (`À moi`, `En cours`, `Fait (24 h)`, `cockpit::Show`) and `Label` (the labels the cards carry, the most used first, a dot of its color; no group when no card has one). A chosen value shows as a chip `Espace : Claap` with its cross, and the control turns into a `+`; the cross, or picking the chosen value again, removes it. No chip is the whole list: every space, `À moi`. The menu lives outside the head (`#fm`, like the snooze menu), filled from the head's hidden `#fmsrc`, so a push refills it open instead of closing it; Escape or a click elsewhere closes it. A new dimension is one more group in `filters_html` (and a value of `Show` is a line in `SHOWS`).

- `À moi` (default): the list as above, what waits on the user. Its menu item counts the cards the scope keeps.
- `En cours`: only what still moves, read from `attn inflight --json` (`[attention] inflight_command`, default `command`'s program with `inflight --json`), cached and refreshed on a thread every 15 s (`crate::inflight`). Its menu item counts what it lists; the sort label reads `En direct`.
  - `Tes clics`: the clicks of the last 24 h not over yet (Valider, Corriger, Consigne), newest first, with a tracker `Envoyé`, `Reçu`, `Fait` and their times. `Reçu` is the `delivered` op of the click; `Fait` is the agent closing the card (`close`, or `settled` when the user had already cleared it) or updating it (`update`). A click done, or a removal (`Retiré`), leaves at once for `Fait (24 h)`. Not received after 3 min: orange, `Pas encore reçu <n> min`. The scope keeps one side's clicks.
  - `Agents au travail pour toi`: liveness is read, not self-reported. A session a Kova pane holds is listed while its pane works, whatever its last status says; a session with no pane, while its `working` status is under 30 min old. Its status text shows only while that status says `working`; its open consignes show under it. Consignes held by sessions at rest are one count line; holding a consigne never lists a session.
- `Fait (24 h)`: the clicks over in the last 24 h (done or `Retiré`), newest first, with their result.

A tile, folded (Robin, 11/11/2026: the tiles said too much): its title, its
label, its source and its age, nothing under the title. What is expected
reads in the title, `<Subject> · <what is expected>`, and under it once the
card is open. The titles Kova composes follow that form: a decision sheet
is `<Subject> · <N> décisions` and nothing else (`decision_sheet::tile_title`:
one count, whatever the sheet's title already said, never its first
question; `kova ask --decisions` posts it so, and an older sheet posted with
a colon reads the same), a permission `<pane> · permission à donner`, a
stopped turn `<pane> · tour arrêté`, a failed routine `Routine <name> · en
échec`. A question's tile is the first line of its message. An attention
card's title is its agent's words, shown as written.

A card's space (`Claap`, `Perso`) and its label are pills filled with their
color (no dot), its name in near black or white,
whichever reads best on it (`cockpit::ink_on`, WCAG luminance): the name
says what it is, nothing rests on the color alone.

A card's label (`attn add|update --label`, a closed registry of ten in `claude-config/labels.json`; `attn list --json` adds `label_color`) shows as a tag tinted with its color next to the space tag. Only a `#RRGGBB` color reaches the page; another reads gray.

The Cockpit page runs down over the global status bar: its shortcuts and counters belong to the terminal.

## Catch-up band

`Depuis ton passage de <HHhMM>` and `<n> projets ont bougé`, between the
title and the filter (`src/catchup.rs`, pure render tested): what moved in
each project since the user last left the page, from `[attention]
catchup_command` (default: `command`'s program with `catchup --json`). Read
off the tick on a thread when the page opens, then at most every 60 s. Nothing
shows when no project moved or before the first read lands.

- Up to 5 rows (project, headline, `À toi : <first waiting title>` in the warn
  colour, the session with a terminal icon), then `Voir plus (n)`. The chevron
  folds the band to its header.
- A click on a row opens it in place: its changes (time, words, the `action`
  and `src` links the side panel can show, opening there like a card's links,
  only links the band holds) and its waiting items. A waiting item that is a
  card of the list reads `Voir la carte`: the list makes room for it (filter
  back to `Tout`, `Voir plus` as needed), scrolls to it and lights it; its
  actions stay on the card.
- Read marking (Robin, 06/11/2026): leaving the page (closing it, Escape, a
  tile, Cmd+J, closing its window, quitting Kova) and `Tout lu` run
  `[attention] seen_command` (default: the program with `seen`, now). Leaving
  marks read only when a read landed during the visit. The band shows what
  was there at open plus what the 60 s refresh brings; something that moved in
  the last minute before leaving is marked read unseen.

## Record

The theme button (header, right of `Chat`) shows the app's appearance (sun
Clair, moon Sombre, half disc Système) and switches to the next one, through
the same `app::set_theme` as View > Theme and `cycle-theme`; its tooltip names
the next mode and the shortcut from `[keys] cycle_theme`.

`Enregistrer` (header, left of `Chat`) records for the montre-une-fois skill
(`src/record.rs`, phases and commands tested): Kova runs macOS's own
`/usr/sbin/screencapture -v -g -k -x <file>` (main display, default
microphone, clicks shown, no sound) into
`~/Movies/Kova/montre-une-fois-<YYYYMMDD-HHMM>.mov` (`-2`, `-3` for the same
minute). Only the click starts it; the state is in memory, so a launch never
records.

- While it records the button reads `Arrêter` with the elapsed time, in red,
  and the sidebar's Cockpit entry shows a red dot in place of its icon.
- `Arrêter` sends SIGINT and waits for the exit (a minute at most) and for the
  file to be non-empty with a stable size. A screencapture that ends on its own
  is handled the same way; quitting Kova stops it.
- Then `attn add --scope=pro --level=memo --kind=info --from=kova
  --topic=montre-une-fois --title=… --body=<path>` (`[attention] command`'s
  program) and `act_command <id> instruct --text=Lance le skill
  montre-une-fois sur la vidéo <path> (…)`, the path of a card's instruction,
  which types it into claap-agent. The note under the title reads
  `Enregistrement remis à claap-agent` with the path (click: `open -R`, shown
  in the Finder) and what `attn act` said.
- No start, or a missing or empty file: `Autorise Kova dans Réglages Système >
  Confidentialité : Enregistrement de l'écran et Microphone, puis réessaie`.
  A hand-off that fails says so with the CLI's words; the file stays.

## Opened cards

A click on any card's body (not a button, a field or the open detail) opens
its detail inside the card, or folds it; `Brief`, `Décider`, `Tester` and
`Répondre` open it too and give the keyboard to its `Consigne` or its answer
(never fold it). Buttons work without opening. The opened keys live in the window
(`CockpitSlot::expanded`): the 250 ms refresh rebuilds the detail and keeps it
open, with its folded prompt, its scroll and a typed answer; a key whose card
is gone is dropped.

- Failed routine (the Scheduled tile's body): `Horaire`, `Prochain`,
  `Dernier` (crit when it failed, with its exit code), `Dossier`; the prompt
  file, folded and scrollable (16 KiB at most); `Lancer maintenant` (or
  `Reprendre` when off), `Mettre en pause`, `Modifier le prompt` as links
  (`Voir le log` is the card's blue button), through the tile's own path (`sidebar_sched_hit`). No delete here.
  An action in flight shows `En cours`; an open that failed shows its words.
- Permission: the prompt's lines and question; `Refuser` / `Autoriser` stay
  on the card.
- Error: the provider's words and the pane's last 15 non-blank lines; the
  card's `Voir le pane` closes the page and focuses the pane.
- Attention entry: `needs`, `body` (line breaks kept), `detail`, the `src`
  list, and under it the `action` (not a `kova-sheet:`) plus the http(s)
  links of the sources, opening in the side panel like `Voir`. The links
  inside the title, needs, body and detail (a URL, a local file, `CLA-123`,
  `#12345`) are in the text itself (`docs/links.md`). An entry with no `needs`, `body` or `detail` says `Je regarde de
  quoi il s'agit…` and Kova runs `explain_command ID` (default: the attn
  program with `explain`, about 40 s) in the background, never twice at once
  for one id; the next read brings the `detail` it saved. A source naming a
  local file the side panel opens (md, html, image, pdf, txt, log, json; the
  path may hold spaces and words may follow it) is a link named by the file,
  its full path as the tooltip, never a dead path; other sources stay words.
- Every opened attention card, button or not, ends with a `Consigne` field (`Une consigne pour cette carte`): Enter runs `act_command ID instruct --text=<text>` (one argv element, `attention::clean_text`), which hands it to the chief of the card's scope; Shift+Enter is a new line, nothing typed sends nothing. The field clears and the card shows `Consigne envoyée` with the text sent; on a draft it sits under the draft's actions, apart from the correction, and never marks the draft sent. A typed instruction survives the refresh.
- Decision sheet: its card's text (the decisions); `Décider` opens it.
- Question: its text and its answers as buttons (a free answer brings a
  field), checked with `ask::accepts_external_answer` and queued like the
  chat's (`queue_ask_action`); the card shows `En cours` for 5 s.

A failed routine has no `Needs attention` section in the Scheduled list any
more (06/11/2026): its row sits in its normal group and says it failed; the
Scheduled icon keeps its red dot.

## Reply zone

When the queue lists `gestures` on a card (`attn list --open --json`), the card answers through one zone instead of its former buttons. A queue that lists none keeps the former card unchanged: the cutover is the queue's, there is no setting.

- One field, `Ta réponse`, and the gestures listed, each a worded button with an icon: `No go` (outlined, a cross), `Demander` (a link, a bubble), `Go` (filled, a check; the queue's `go_label`, `Go, envoyer` on a draft). Only the gestures listed show.
- `Go` and `No go` send the text typed, if any, and file the card. `Demander` needs a text: the button is disabled while the field is empty. Nothing is posted from the keyboard: Enter is a new line.
- `Vu` sits at the card's head, where the check and the cross were: filed with no judgement. The clock stays.
- A card with a draft shows its zone at once; the draft is read only (a change is asked with `Demander`) and `Go` carries its fingerprint (`--seen`). Another card shows its zone once opened; folded, its button opens it there (`Répondre`, or `Brief`, `Décider`, `Tester` by kind).
- A card that waits for its agent (`waiting: "agent"`, `ask`) says so at its head, folded or not: `Attend l'agent`, the delivery in the queue's own words (`non remis`, `sera lu à son prochain pas`), the question asked, and `Annuler ma demande` when `cancel` is listed. It has no check, no cross and no `Vu`.
- Each gesture runs `act_command --exit-codes --client-id=<uuid> <id> <go|nogo|ask|seen|cancel> [--text=<text>] [--seen=<fingerprint>]` (`attention::gesture_argv`). The id is new per click and kept when the same gesture is clicked again after a failure. What the card shows next is read from the queue; a failure or a refusal leaves the command's line on the card, and a question that did not go is kept under the field.

## Chat

A chief of staff's chat sits on the left third (the side panel keeps the
right), open by default. `Chat` (top right), the column's X and Cmd+Option+T
(`toggle_chat`, IPC `dispatch-action` `toggle-chat`) hide or show it; the
choice holds for the window's life. From outside the Cockpit, Cmd+Option+T
opens it on its chat. The segmented `Claap | Perso` picks the pane of this
window whose agent session or peer name is `claap-agent` or `perso-agent`.
It is the chat's own view (`chat_view.rs`, `docs/chat-mode.md`):
`cockpit_chat_target` gives that pane and the column's frame while the
Cockpit is open, and nothing moves the focus or switches tab to it. No chief pane: the column says so in one line (no launch yet).
The chosen chief is saved per window in the session file (`cockpit_chief`).

## Une entrée de la file qui renvoie à une fiche

Une entrée dont l'`action` est `kova-sheet:<id>` (ou `kova-ask:<id>`, id complet ou préfixe d'au moins 6 caractères) d'une question `kova ask` en attente porte le bouton « Décider » : il ouvre la fiche dans le volet, images comprises. La carte propre de la question n'apparaît plus, pour ne la montrer qu'une fois. Pas de « C'est fait » : l'entrée part quand la fiche est répondue et l'entrée fermée.
