# kova render: the pages without the app

`kova render` writes the Cockpit, the chat and the decision sheets as HTML
files, with no window and without a running Kova. The pages are the app's
own: the same Rust builds them (`cockpit::parts`, `chat::page::entries`,
`decision_sheet::render`), in the same shell with the same script. What the
web view adds (clicks reaching Kova, the live data) is not there.

For an agent: look at what a change shows, and attach a before and an after
to the work, without asking the user to launch anything. Code:
`src/render_cli.rs`.

## Commands

```
kova render cockpit --list <json> --out <dir>
      [--inflight <json>] [--panes <json>] [--catchup <json>] [--open <card id>] [--light]
      [--state <card id>=delivered|noted|running|failed:<why>]...
kova render chat --transcript <jsonl> --out <file.html>
      [--markers "a|b"] [--base <the pane's folder>] [--exchanges N] [--unfold] [--light]
kova render sheet --sheet <json> --out <file.html> [--from <who>] [--light]
kova render question --message <text or @file> --out <file.html>
      [--from <who>] [--button <label>]... [--input] [--light]
```

Each prints the files it wrote. Exit 0 when written, 2 on a bad call (the
reason on stderr, nothing written).

| Page | Reads | Writes |
|---|---|---|
| `cockpit` | what the `[attention]` commands print, saved to files: the queue (`command`, e.g. `attn list --open --json --scope all`), the clicks in flight (`inflight_command`), the catch-up (`catchup_command`); `--panes` is `[["session name", working], ...]` | `default.html` (the cards), `menu.html` (the filter menu open), `chips.html` (one space, En cours), `en-cours.html`, `fait.html`, and `card.html` (the card `--open` names, opened with its detail). With `--catchup`, the band is on every page, unfolded, every project opened. `--state` shows a card as a click left it (handed to the agent, recorded, running, failed with its reason) |
| `chat` | a Claude transcript, read as the chat reads it (the window from the end, widened until it holds the exchanges shown) | the thread. `--markers` is `[chat] rewrite_markers`, `--base` the folder relative paths are read against, `--unfold` opens the `Version précédente` rows. Prints what it read; exit 1 when a text the assistant wrote in the exchanges shown is not on the page |
| `sheet` | a decision sheet file (`kova ask --decisions`) | the sheet as the side panel shows it, unanswered |
| `question` | a long `kova ask` message | the question as the side panel shows it |

Not rendered: what Kova draws natively (the terminal, the sidebar, the
floating `kova ask` card), and the Cockpit's chat column inside the Cockpit
page (`chat` renders it on its own).

## A screenshot

`scripts/render-shot.sh <file.html> <file.png> [width,height]` runs a
headless Chrome on the file (default 1100,1400; the chat column is about
560 wide). Nothing shows on screen. The image is the window's size: give a
height that holds what must be seen.

## Before and after

```sh
kova render cockpit --list list.json --inflight inflight.json --out before/   # on main
kova render cockpit --list list.json --inflight inflight.json --out after/    # on the branch
scripts/render-shot.sh before/en-cours.html before.png
scripts/render-shot.sh after/en-cours.html after.png
```

Same data both times: save the JSON once. From a worktree whose build is
not installed, `cargo run --release -- render …` runs the same command
(it never opens the app).
