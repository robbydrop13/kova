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
  place (see Opened cards); `Ouvrir la fiche` for a decision sheet (no `C'est fait`); `C'est fait`
  runs `close_command`; `Refuser` / `Autoriser` answer a permission prompt
  like the chat mode does, only if the screen still shows the prompt the card
  was rendered from.
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

## Opened cards

`Ouvrir` on a failed routine, a permission, an error or a plain question opens
its detail inside the card, with no jump to another view; the button reads
`Masquer` while it is open. The opened keys live in the window
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
- Question: its text and its answers as buttons (a free answer brings a
  field), checked with `ask::accepts_external_answer` and queued like the
  chat's (`queue_ask_action`); the card shows `En cours` for 5 s.

A failed routine has no `Needs attention` section in the Scheduled list any
more (06/11/2026): its row sits in its normal group and says it failed; the
Scheduled icon keeps its red dot.

## Chat

`Chat` (top right) opens a chief of staff's chat on the right third: the
segmented `Claap | Perso` picks the pane of this window whose agent session or
peer name is `claap-agent` or `perso-agent`. It is the chat mode's own view
(`chat_view.rs`): `chat_target` returns that pane and the column's frame while
the Cockpit is open, and counts it as focused, so nothing moves the focus or
switches tab. No chief pane: the column says so in one line (no launch yet).
The chosen chief is saved per window in the session file (`cockpit_chief`).

## Une entrée de la file qui renvoie à une fiche

Une entrée dont l'`action` est `kova-sheet:<id>` (ou `kova-ask:<id>`, id complet ou préfixe d'au moins 6 caractères) d'une question `kova ask` en attente porte le bouton « Ouvrir la fiche » : il ouvre la fiche dans le volet, images comprises. La carte propre de la question n'apparaît plus, pour ne la montrer qu'une fois. Pas de « C'est fait » : l'entrée part quand la fiche est répondue et l'entrée fermée.
