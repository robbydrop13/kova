//! `kova render`: the Cockpit, the chat and the decision sheets written as
//! HTML files, with no window and no running Kova, from the data the app
//! reads (`docs/render.md`). The pages are the app's own: same Rust, same
//! shell, same script. `scripts/render-shot.sh` turns a file into a PNG.
//!
//! For an agent to look at what a change shows, and to attach a before and
//! an after to its work, without the user launching anything.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::attention::Scope;
use crate::chat::conversation::{BlockKind, Conversation, Item};
use crate::chat::page;
use crate::cockpit::{self, Filter, Show, View};
use crate::window::sidebar::tokens;

pub const USAGE: &str = "\
kova render: write Kova's pages as HTML files, without a window

  kova render cockpit --list <json> --out <dir>
        [--inflight <json>] [--panes <json>] [--catchup <json>] [--open <card id>] [--light]
        [--state <card id>=delivered|noted|running|failed:<why>]... [--refs <json>]
        [--journal <file.md>] [--ask <question.json>]...
      <json> files hold what the `[attention]` commands print: `attn list --open --json`,
      `attn inflight --json`, `attn catchup --json`; panes: [[\"session\", working], ...].
      Writes default.html, menu.html, chips.html, en-cours.html, fait.html, and
      card.html (the card opened) with --open.

  kova render chat --transcript <jsonl> --out <file.html>
        [--markers \"a|b\"] [--base <the pane's folder>] [--exchanges N] [--unfold] [--light]
        [--refs <json>]
      The thread as the chat reads it. Exit 1 when a text the assistant wrote
      in the exchanges shown is not on the page.

  kova render sheet --sheet <json> --out <file.html> [--from <who>] [--light]
      A decision sheet (`kova ask --decisions`).

  kova render question --message <text or @file> --out <file.html>
        [--from <who>] [--button <label>]... [--input] [--light]
      A long `kova ask` question as the side panel shows it.

The pages are what the app would show: `[chat] rewrite_markers` and `[links] refs_file`
come from Kova's config unless --markers or --refs is given; --no-config ignores it.

Then: scripts/render-shot.sh <file.html> <file.png> [width,height]";

struct Args {
    flags: Vec<(String, Option<String>)>,
}

/// Flags that take no value.
const SWITCHES: &[&str] = &["--light", "--unfold", "--input", "--no-config"];

impl Args {
    fn parse(args: &[String]) -> Result<Args, String> {
        let mut flags = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if !a.starts_with("--") {
                return Err(format!("unexpected '{a}'"));
            }
            if SWITCHES.contains(&a.as_str()) {
                flags.push((a.clone(), None));
                i += 1;
            } else {
                let v = args.get(i + 1).ok_or_else(|| format!("{a} needs a value"))?;
                flags.push((a.clone(), Some(v.clone())));
                i += 2;
            }
        }
        Ok(Args { flags })
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.flags.iter().find(|(n, _)| n == name).and_then(|(_, v)| v.as_deref())
    }

    fn all(&self, name: &str) -> Vec<String> {
        self.flags.iter().filter(|(n, _)| n == name).filter_map(|(_, v)| v.clone()).collect()
    }

    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(n, _)| n == name)
    }

    fn need(&self, name: &str) -> Result<&str, String> {
        self.get(name).ok_or_else(|| format!("{name} is required"))
    }

    fn palette(&self) -> &'static tokens::Palette {
        if self.has("--light") { &tokens::LIGHT } else { &tokens::DARK }
    }
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
}

fn write(path: &Path, html: &str) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, html).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("{}", path.display());
    Ok(())
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `kova render …`: the exit code.
pub fn run(args: &[String]) -> i32 {
    let Some(what) = args.first().map(String::as_str) else {
        eprintln!("{USAGE}");
        return 2;
    };
    if matches!(what, "--help" | "-h" | "help") {
        println!("{USAGE}");
        return 0;
    }
    let done = Args::parse(&args[1..]).map(|mut a| {
        // What the app would show: the user's config gives what a flag does
        // not (`[chat] rewrite_markers`, `[links] refs_file`). `--no-config`
        // for a page that depends on the flags alone.
        if !a.has("--no-config") {
            let config = crate::config::Config::load();
            if !a.has("--markers") && !config.chat.rewrite_markers.is_empty() {
                a.flags.push(("--markers".to_string(), Some(config.chat.rewrite_markers.join("|"))));
            }
            if !a.has("--refs") && let Some(file) = config.links.refs_path() {
                a.flags.push(("--refs".to_string(), Some(file.to_string_lossy().into_owned())));
            }
        }
        // The ids' titles (`[links] refs_file`), for the pages that draw them.
        crate::refs::set_file(a.get("--refs").map(Path::new));
        a
    });
    let done = done.and_then(|a| match what {
        "cockpit" => render_cockpit(&a),
        "chat" => render_chat(&a),
        "sheet" => render_sheet(&a),
        "question" => render_question(&a),
        other => Err(format!("'{other}' is not a page (cockpit, chat, sheet, question)")),
    });
    match done {
        Ok(code) => code,
        Err(why) => {
            eprintln!("kova render: {why}");
            2
        }
    }
}

fn render_cockpit(a: &Args) -> Result<i32, String> {
    let dir = PathBuf::from(a.need("--out")?);
    let entries = crate::attention::parse(&read(a.need("--list")?)?).ok_or("--list: not what `attn list --json` prints")?;
    let data = match a.get("--inflight") {
        Some(f) => crate::inflight::parse(&read(f)?).ok_or("--inflight: not what `attn inflight --json` prints")?,
        None => crate::inflight::InFlight::default(),
    };
    let panes: Vec<crate::inflight::Pane> = match a.get("--panes") {
        Some(f) => serde_json::from_str(&read(f)?).map_err(|e| format!("--panes: {e}"))?,
        None => Vec::new(),
    };
    let catchup = match a.get("--catchup") {
        Some(f) => Some(crate::catchup::parse(&read(f)?).ok_or("--catchup: not what `attn catchup --json` prints")?),
        None => None,
    };
    // Where a click on a card stands, as `attn act --exit-codes` would leave it.
    let mut states: std::collections::HashMap<String, crate::attention::Outcome> = std::collections::HashMap::new();
    for s in a.all("--state") {
        use crate::attention::Outcome;
        let (id, state) = s.split_once('=').ok_or("--state: <card id>=<state>")?;
        let outcome = match state {
            "delivered" => Outcome::Sent,
            said if crate::attention::said_line(said).is_some() => match crate::attention::said_line(said) {
                Some(crate::attention::Delivery::Said(line)) => Outcome::Said(line),
                _ => Outcome::SentByCode(said.to_string()),
            },
            "noted" => Outcome::Noted,
            "running" => Outcome::Running,
            other => match other.strip_prefix("failed:") {
                Some(why) => Outcome::Failed { words: why.to_string(), until_ms: 0 },
                None => return Err(format!("--state: '{other}' is not delivered, noted, running or failed:<why>")),
            },
        };
        states.insert(id.to_string(), outcome);
    }
    // MOCK-UP ONLY: `--reply-mock a|b`, `--asked <id>=<question>`.
    cockpit::REPLY_MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.0 = a.get("--reply-mock").and_then(|v| v.chars().next());
        m.1 = a.all("--asked").iter().filter_map(|s| s.split_once('=')).map(|(id, q)| (format!("attention:{id}"), q.to_string())).collect();
    });
    let mut items: Vec<cockpit::Item> = entries.iter().filter_map(|e| cockpit::from_attention(e, states.get(&e.id), true)).collect();
    // Waiting asks, each from its file of the board's store: their tiles as
    // the Cockpit makes them (`from_need`, then `ask_title`).
    for file in a.all("--ask") {
        let q: crate::ask::Question = serde_json::from_str(&read(&file)?).map_err(|e| format!("--ask {file}: {e}"))?;
        // As in the app: an entry that stands for this ask (`kova-sheet:<id>`,
        // `kova-ask:<id>`) shows it, the ask's own tile is left out.
        let stood_for = entries.iter().filter_map(|e| e.action.as_deref()).any(|act| {
            ["kova-sheet:", "kova-ask:"].iter().any(|p| act.strip_prefix(p).is_some_and(|id| id == q.id))
        });
        if stood_for {
            continue;
        }
        let sheet = q.panel_link().is_some() && !q.is_long();
        let need = crate::window::needs::NeedItem {
            kind: crate::window::needs::NeedKind::Ask,
            key: format!("ask:{}", q.id),
            pane_id: None,
            since: q.asked_at,
            title: q.from.name.clone(),
            detail: q.message.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" "),
        };
        if let Some(mut item) = cockpit::from_need(&need, sheet) {
            item.title = cockpit::ask_title(&q.message, sheet);
            items.push(item);
        }
    }
    cockpit::sort(&mut items);
    let now = now();
    let light = a.has("--light");
    // The band unfolded, every project opened: all its text is on the page.
    let band = catchup.as_ref().map_or(String::new(), |data| {
        let opened: HashSet<String> = data.projects.iter().map(|p| p.project.clone()).collect();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let exists = |p: &Path| p.is_file();
        let opens = |l: &str| crate::window::side_panel::open_target(l, home.as_deref(), &exists).is_some();
        let is_card = |k: &str| items.iter().any(|i| i.key == k);
        crate::catchup::band_html(&crate::catchup::Band {
            data,
            folded: false,
            opened: &opened,
            all: true,
            is_card: &is_card,
            opens: &opens,
            clock: &crate::catchup::local_clock,
        })
    });
    let page = |name: &str, items: &[cockpit::Item], filter: Filter, show: Show, script: &str| -> Result<(), String> {
        let scope = match filter {
            Filter::All => None,
            Filter::Only(s) => Some(s),
        };
        let inflight = match show {
            Show::InFlight => crate::inflight::list_html(&data, scope, &panes, now, &crate::catchup::local_clock),
            Show::Done => crate::inflight::done_html(&data, scope, now, &crate::catchup::local_clock),
            Show::Mine => String::new(),
        };
        let view = View {
            items,
            filter,
            chat: None,
            can_act: true,
            can_close: true,
            can_snooze: true,
            limit: items.len().max(cockpit::PAGE_SIZE),
            now,
            band: band.clone(),
            record: String::new(),
            record_note: String::new(),
            theme: String::new(),
            show,
            inflight,
            inflight_n: crate::inflight::count(&data, scope, &panes, now),
            done_n: crate::inflight::done_count(&data, scope, now),
            journal: a.get("--journal").map(str::to_string),
            label: None,
        };
        let (head, body, chat) = cockpit::parts(&view);
        let update = cockpit::Update { head: Some(head), list: Some(body), chat: Some(chat), focus: false, reveal: None };
        let html = cockpit::shell(light, "render")
            .replace("</body>", &format!("<script nonce=\"render\">{}{script}</script></body>", cockpit::update_script(&update)));
        write(&dir.join(name), &html)
    };
    page("default.html", &items, Filter::All, Show::Mine, "")?;
    page("menu.html", &items, Filter::All, Show::Mine, ";document.querySelector('[data-fopen]').click()")?;
    page("chips.html", &items, Filter::Only(Scope::Pro), Show::InFlight, "")?;
    page("en-cours.html", &items, Filter::All, Show::InFlight, "")?;
    page("fait.html", &items, Filter::All, Show::Done, "")?;
    let opened = a.all("--open");
    if opened.len() > 1 {
        let mut all: Vec<cockpit::Item> = Vec::new();
        for id in &opened {
            let entry = entries.iter().find(|e| &e.id == id).ok_or_else(|| format!("--open: no card {id} in the list"))?;
            if let Some(mut item) = cockpit::from_attention(entry, states.get(&entry.id), true) {
                item.detail = Some(cockpit::entry_detail(entry, false));
                all.push(item);
            }
        }
        page("cards.html", &all, Filter::All, Show::Mine, "")?;
    }
    if let Some(id) = a.get("--open") {
        let entry = entries.iter().find(|e| e.id == id).ok_or_else(|| format!("--open: no card {id} in the list"))?;
        let mut one: Vec<cockpit::Item> = cockpit::from_attention(entry, states.get(&entry.id), true).into_iter().collect();
        if let Some(item) = one.first_mut() {
            item.detail = Some(cockpit::entry_detail(entry, false));
        }
        page("card.html", &one, Filter::All, Show::Mine, "")?;
    }
    Ok(0)
}

fn render_chat(a: &Args) -> Result<i32, String> {
    let from = a.need("--transcript")?;
    let out = PathBuf::from(a.need("--out")?);
    let markers: Vec<String> = a.get("--markers").unwrap_or_default().split('|').map(str::to_string).collect();
    let exchanges = match a.get("--exchanges") {
        Some(n) => n.parse::<usize>().map_err(|_| "--exchanges: a number".to_string())?.max(1),
        None => page::EXCHANGES_STEP,
    };
    if !Path::new(from).is_file() {
        return Err(format!("{from}: no such transcript"));
    }
    // Read as the chat does: the window from the end, widened until it
    // holds the exchanges shown.
    let began = std::time::Instant::now();
    let (mut tail, mut lines) = crate::chat::tail::Tail::open(Path::new(from));
    let mut conv = Conversation::with_rewrite_markers(&markers);
    conv.apply_all(&lines);
    while conv.user_count() < exchanges && tail.can_widen() {
        lines = tail.widen();
        conv = Conversation::with_rewrite_markers(&markers);
        conv.apply_all(&lines);
    }
    let read_ms = began.elapsed().as_millis();
    crate::links::set_base(a.get("--base").map(Path::new));
    let (entries, more) = page::entries(&conv, exchanges, false);
    crate::links::set_base(None);
    let html: String = entries.iter().map(|e| e.html.as_str()).collect();
    let users: Vec<usize> = conv.items.iter().enumerate().filter(|(_, i)| matches!(i, Item::User { .. })).map(|(n, _)| n).collect();
    let start = if users.len() > exchanges { users[users.len() - exchanges] } else { 0 };
    // Each text the assistant wrote has its entry: shown (`t:`) or folded
    // as a previous version (`v:`). By id, not by rendering the text again:
    // the ids' titles come from a cache that may change in between.
    let ids: std::collections::HashSet<&str> = entries.iter().map(|e| e.id.as_str()).collect();
    let (mut texts, mut missing) = (0, 0);
    for item in &conv.items[start..] {
        let Item::Assistant { id, blocks, .. } = item else { continue };
        for (k, block) in blocks.iter().enumerate() {
            if let BlockKind::Text(t) = &block.kind {
                texts += 1;
                if !ids.contains(format!("t:{id}:{k}").as_str()) && !ids.contains(format!("v:{id}:{k}").as_str()) {
                    missing += 1;
                    eprintln!("kova render: a text is not on the page: {:?}", t.chars().take(120).collect::<String>());
                }
            }
        }
    }
    let update = page::Update { entries: Some(page::wire(&entries, &mut std::collections::HashMap::new())), more, first: true, ..Default::default() };
    let unfold = if a.has("--unfold") { r#";document.querySelectorAll(".prev > .trow").forEach((b) => b.click())"# } else { "" };
    let script = format!(r#"<script nonce="render">{}{unfold}</script></body>"#, page::update_script(&update).replace("</", "<\\/"));
    let theme = crate::window::chat_view::theme_of(a.palette());
    write(&out, &page::shell(&theme, "chat", "render").replace("</body>", &script))?;
    println!(
        "read {} lines in {read_ms} ms: {} exchanges, {} shown, {} entries, {texts} texts ({missing} missing), {} file links, {} folded drafts",
        lines.len(),
        conv.user_count(),
        users.len().min(exchanges),
        entries.len(),
        html.matches("data-path").count(),
        html.matches("Version précédente").count(),
    );
    Ok(if missing == 0 { 0 } else { 1 })
}

fn render_sheet(a: &Args) -> Result<i32, String> {
    let sheet = crate::decision_sheet::Sheet::from_json(&read(a.need("--sheet")?)?)?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let html = crate::decision_sheet::render(&sheet, &Default::default(), a.get("--from").unwrap_or("agent"), a.palette(), home.as_deref());
    write(Path::new(a.need("--out")?), &html)?;
    Ok(0)
}

fn render_question(a: &Args) -> Result<i32, String> {
    let message = a.need("--message")?;
    let message = match message.strip_prefix('@') {
        Some(file) => read(file)?,
        None => message.to_string(),
    };
    let html = crate::decision_sheet::render_question(a.get("--from").unwrap_or("agent"), &message, &a.all("--button"), a.has("--input"), a.palette());
    write(Path::new(a.need("--out")?), &html)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("kova-render-{}", crate::ask::new_question_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_transcript_renders_to_one_page_with_its_texts() {
        let d = dir();
        let out = d.join("chat.html");
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/transcripts/stop_hook_form_rewrite.jsonl");
        let code = run(&argv(&["chat", "--transcript", fixture, "--out", &out.to_string_lossy(), "--markers", "Rewrite it in full", "--no-config"]));
        assert_eq!(code, 0);
        let html = std::fs::read_to_string(&out).unwrap();
        assert!(html.contains("Version précédente") && html.contains(r#"<script nonce="render">"#));
        assert_eq!(html.matches("<script").count(), 2, "the page's script and the data");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn the_cockpit_renders_its_pages_from_the_queue_s_json() {
        let d = dir();
        let list = d.join("list.json");
        std::fs::write(&list, r#"[{"id":"a-1","level":"now","kind":"decision","from":"pm","title":"Pick a plan","needs":"Say A or B","ts":"2026-11-10T10:00:00+01:00"}]"#).unwrap();
        let out = d.join("pages");
        let code = run(&argv(&["cockpit", "--list", &list.to_string_lossy(), "--out", &out.to_string_lossy(), "--open", "a-1", "--no-config"]));
        assert_eq!(code, 0);
        for page in ["default.html", "menu.html", "chips.html", "en-cours.html", "fait.html", "card.html"] {
            assert!(out.join(page).is_file(), "{page}");
        }
        assert!(std::fs::read_to_string(out.join("default.html")).unwrap().contains("Pick a plan"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_bad_call_says_why_and_writes_nothing() {
        assert_eq!(run(&argv(&[])), 2);
        assert_eq!(run(&argv(&["help"])), 0);
        assert_eq!(run(&argv(&["chat", "--out", "/tmp/x.html", "--no-config"])), 2, "no transcript");
        assert_eq!(run(&argv(&["chat", "--transcript", "/nonexistent.jsonl", "--out", "/tmp/kova-render-none.html", "--no-config"])), 2);
        assert!(!Path::new("/tmp/kova-render-none.html").exists());
        assert_eq!(run(&argv(&["terminal"])), 2);
    }
}
