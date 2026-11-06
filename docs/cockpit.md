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
- Buttons: `Ouvrir` for an entry whose `action` the side panel can show
  (`sidebar_attention::open_target`); `Ouvrir` on a live need opens the card in
  place (see Opened cards); `Ouvrir la fiche` for a decision sheet; `Refuser` / `Autoriser` answer a permission prompt
  like the chat does, only if the screen still shows the prompt the card
  was rendered from.
- Waiting on a go (Robin, 06/11/2026): an entry without a draft of kind
  `would` or `review`, or a `decision` whose `action` is prepared (not a
  `kova-sheet:` / `kova-ask:`, answered on its sheet), carries `Refuser` and
  `Valider`, both blue, after `Ouvrir` (secondary then). `Valider` runs
  `act_command ID Valider` (the chief of the scope receives it) and the card
  reads `Transmis`. `Refuser` opens a small field `Pourquoi ? (facultatif)`
  under the card: Enter runs `close_command ID --reason dismissed`, plus
  `--note=<reason>` when one was typed (one argv element), and the card
  leaves; Escape folds the field. No close command, no `Refuser`. A draft
  keeps its own `Corriger` / `Valider`.
- Icons (every attention card, top right, Feather, with a tooltip): clock
  `Plus tard`, x `Pas pertinent, retirer` (`close_command ID --reason
  dismissed`), check `C'est fait` (`close_command ID --reason done`). The clock
  opens a small menu in the page: `Dans 1 h`, `Ce soir` (19:00, or tomorrow
  19:00 once past), `Demain matin` (09:00), `Lundi prochain` (09:00),
  `Choisir…` (a date and time field); it runs `snooze_command ID
  --until=<ISO 8601 with offset>` (default: `close_command`'s program with
  `snooze`). The card leaves the page at once and stays hidden until the queue
  drops it (60 s at most); a failed command brings it back with its words.
  Live needs keep their own buttons, no icons.
- `Voir plus`: the list shows the first 10 cards in the priority order, then
  `Voir plus (N)` (N left) adds 10 more. Section and filter counts cover the
  whole list. Opening the page or changing the filter starts again at 10.
- Drafts: an entry with a `draft` shows it in an editable field under
  `À <to>  <channel>  Objet <subject>`. `Valider` runs `act_command ID
  Valider`; once the text is rewritten it reads `Valider la correction` and
  runs `act_command ID Corriger --text=<text>`, the text one argv element
  (`attention::clean_text`: no control character but tab and newline, 16 KiB
  at most). `Corriger` focuses the field, `Annuler` puts the draft back. Kova
  never sends anything: the chief who receives the click does.

Every text is escaped into the HTML in Rust. The page's script (CSP nonce)
only posts clicks, the filter and the edited text (`kovaCockpit`); messages
from another frame, another URL or an earlier load are dropped. The tick
gathers the items every 250 ms and pushes only the parts that changed; a draft
being rewritten keeps its text, caret and focus through a push.

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
its detail inside the card, or folds it; `Ouvrir` on a failed routine, a
permission, an error or a plain question does the same and reads `Masquer`
while it is open. Buttons work without opening. The opened keys live in the window
(`CockpitSlot::expanded`): the 250 ms refresh rebuilds the detail and keeps it
open, with its folded prompt, its scroll and a typed answer; a key whose card
is gone is dropped.

- Failed routine (the Scheduled tile's body): `Horaire`, `Prochain`,
  `Dernier` (crit when it failed, with its exit code), `Dossier`; the prompt
  file, folded and scrollable (16 KiB at most); `Lancer maintenant` (or
  `Reprendre` when off), `Voir le log`, `Mettre en pause`, `Modifier le
  prompt`, through the tile's own path (`sidebar_sched_hit`). No delete here.
  An action in flight shows `En cours`; an open that failed shows its words.
- Permission: the prompt's lines and question; `Refuser` / `Autoriser` stay
  on the card.
- Error: the provider's words, the pane's last 15 non-blank lines, and `Voir le
  pane`, which closes the page and focuses the pane.
- Attention entry: `needs`, `body` (line breaks kept), `detail`, the `src`
  list, and its links: the `action` (not a `kova-sheet:`) plus every http(s)
  link in the body, detail and sources, opening in the side panel like
  `Ouvrir`. An entry with no `needs`, `body` or `detail` says `Je regarde de
  quoi il s'agit…` and Kova runs `explain_command ID` (default: the attn
  program with `explain`, about 40 s) in the background, never twice at once
  for one id; the next read brings the `detail` it saved.
- Every opened attention card, button or not, ends with a `Consigne` field (`Une consigne pour cette carte`): Enter runs `act_command ID instruct --text=<text>` (one argv element, `attention::clean_text`), which hands it to the chief of the card's scope; Shift+Enter is a new line, nothing typed sends nothing. The field clears and the card shows `Consigne envoyée` with the text sent; on a draft it sits under the draft's actions, apart from the correction, and never marks the draft sent. A typed instruction survives the refresh.
- Decision sheet: its card's text (the decisions); `Ouvrir la fiche` opens it.
- Question: its text and its answers as buttons (a free answer brings a
  field), checked with `ask::accepts_external_answer` and queued like the
  chat's (`queue_ask_action`); the card shows `En cours` for 5 s.

A failed routine has no `Needs attention` section in the Scheduled list any
more (06/11/2026): its row sits in its normal group and says it failed; the
Scheduled icon keeps its red dot.

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

Une entrée dont l'`action` est `kova-sheet:<id>` (ou `kova-ask:<id>`, id complet ou préfixe d'au moins 6 caractères) d'une question `kova ask` en attente porte le bouton « Ouvrir la fiche » : il ouvre la fiche dans le volet, images comprises. La carte propre de la question n'apparaît plus, pour ne la montrer qu'une fois. Pas de « C'est fait » : l'entrée part quand la fiche est répondue et l'entrée fermée.
