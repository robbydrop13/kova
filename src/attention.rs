//! The attention queue the Cockpit lists with the live needs: the command
//! named by `[attention] command` (nothing runs when it is absent), read like
//! `team::snapshot` (cached, refreshed on a background thread, never on the
//! tick), and the two commands a row's buttons run, `act_command` and
//! `close_command`.
//!
//! The CLI owns the queue; Kova only lists it and passes a click on. Every
//! command is an argv from the config with the entry id and fixed words
//! appended, never a shell line. The id is checked before it reaches a
//! `Command`: it must be one the last read returned, and it cannot start with
//! a dash, so an entry can never smuggle an option. See
//! `docs/sidebar-spec.md`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use serde::Deserialize;

/// How long a read stays fresh. The queue changes when an agent adds a line
/// or the user closes one; ten seconds keeps the list honest at the cost of
/// one short process.
pub const MAX_AGE_SECS: u64 = 10;
/// How long a read may take before the child is killed.
const LIST_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);
/// How long a click's command may take.
const ACT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);
/// How long a failed click's words stay on its row, in milliseconds.
pub const FAILED_MS: u64 = 60_000;

/// How urgent an entry is. The view lays the three levels out apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Wanted now: the top section, counted with the questions.
    Now,
    /// For the next review: folded, grouped by topic.
    Point,
    /// For the record: a count line.
    Memo,
}

impl Level {
    fn parse(word: &str) -> Option<Level> {
        match word {
            "now" => Some(Level::Now),
            "point" => Some(Level::Point),
            "memo" => Some(Level::Memo),
            _ => None,
        }
    }
}

/// Which queue an entry comes from: the work one or the personal one
/// (`attn list --scope all` reads both). Absent means work, as before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Pro,
    Perso,
}

/// A reply an agent drafted for the user to approve or correct. Kova shows
/// it and passes the click on; whoever receives the click sends it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Draft {
    #[serde(default)]
    pub to: String,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub subject: Option<String>,
    pub text: String,
    /// Where the message really goes, as the sender's tool takes it (a
    /// Slack id `U…`, `C…`, `D…`, `G…`, a `#channel`, `/<ts>` for a thread):
    /// shown next to `to`, the name a person reads.
    #[serde(default)]
    pub target: Option<String>,
}

/// One open entry, as the CLI prints it. Fields Kova does not show are
/// ignored, so the CLI can grow without breaking the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    /// Epoch seconds the entry was added; 0 when its date cannot be read.
    pub since: u64,
    pub from: String,
    pub level: Level,
    /// `decision`, `review`, `test`, `would`... The buttons depend on it.
    pub kind: String,
    pub title: String,
    /// What is expected of the user, when the entry says.
    pub needs: Option<String>,
    /// A URL, a file path (with an optional `#anchor`), or plain words.
    pub action: Option<String>,
    pub topic: Option<String>,
    pub scope: Scope,
    /// 1 urgent, 2 today, 3 this week; `None` when the entry does not say.
    pub prio: Option<u8>,
    pub draft: Option<Draft>,
    /// The agent's longer words on it, line breaks kept.
    pub body: Option<String>,
    /// What `attn explain` wrote for an entry that said nothing else.
    pub detail: Option<String>,
    /// The sources it cites: ids, paths or links.
    pub src: Vec<String>,
    /// Screenshots before and after a change (`attn add --before/--after`):
    /// absolute paths, shown in the card (Robin, 09/11/2026).
    pub before: Option<String>,
    pub after: Option<String>,
    /// Its label from the closed registry (`attn labels`), with its color.
    pub label: Option<Label>,
    /// The card came back after its action failed (`attn fail`): why.
    pub failed: Option<String>,
    /// The proof that its action took place (a message id, a link, a
    /// commit, a file).
    pub proof: Option<String>,
    /// An action plan the user says go or no go to: a `decision` posted by
    /// `PLAN_FROM` with the plan in its body.
    pub plan: bool,
}

/// The producer whose `decision` cards with a body are action plans.
pub const PLAN_FROM: &str = "veille-actions";

/// A card's label: a name from `attn`'s registry of ten at most and its
/// `#RRGGBB` color (Robin, 10/11/2026).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub name: String,
    pub color: String,
}

/// A `#RRGGBB` color, the only form a label's color takes into the page.
pub fn hex_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit())
}

#[derive(Deserialize)]
struct Raw {
    id: String,
    #[serde(default)]
    ts: String,
    #[serde(default)]
    from: String,
    level: String,
    #[serde(default)]
    kind: String,
    title: String,
    #[serde(default)]
    needs: Option<String>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    prio: Option<serde_json::Value>,
    #[serde(default)]
    draft: Option<serde_json::Value>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    src: Option<serde_json::Value>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    label_color: Option<String>,
    #[serde(default)]
    failed: Option<String>,
    #[serde(default)]
    proof: Option<String>,
}

/// Parse the CLI's answer. `None` when it is not a JSON array at all; an
/// entry that does not read (no id, an unknown level, an unsafe id) is
/// skipped on its own, the rest still shows.
pub fn parse(text: &str) -> Option<Vec<Entry>> {
    let values: Vec<serde_json::Value> = serde_json::from_str(text).ok()?;
    let nonempty = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    Some(
        values
            .into_iter()
            .filter_map(|value| serde_json::from_value::<Raw>(value).ok())
            .filter(|raw| valid_id(&raw.id))
            .filter_map(|raw| {
                // An action plan: said by who posts it and what it is.
                let plan = raw.kind == "decision" && raw.from == PLAN_FROM && raw.body.as_deref().is_some_and(|b| !b.trim().is_empty());
                Some(Entry {
                    plan,
                    failed: nonempty(raw.failed.clone()),
                    proof: nonempty(raw.proof.clone()),
                    level: Level::parse(&raw.level)?,
                    since: epoch_of(&raw.ts).unwrap_or(0),
                    id: raw.id,
                    from: raw.from,
                    kind: raw.kind,
                    title: raw.title,
                    needs: nonempty(raw.needs),
                    action: nonempty(raw.action),
                    topic: nonempty(raw.topic),
                    scope: if raw.scope.as_deref() == Some("perso") { Scope::Perso } else { Scope::Pro },
                    // Read leniently: a priority or a draft that does not
                    // read is none, the entry still shows.
                    prio: raw.prio.and_then(|p| p.as_u64()).filter(|p| (1..=3).contains(p)).map(|p| p as u8),
                    draft: raw
                        .draft
                        .and_then(|d| serde_json::from_value::<Draft>(d).ok())
                        .filter(|d| !d.text.trim().is_empty()),
                    body: nonempty(raw.body),
                    detail: nonempty(raw.detail),
                    // A list of words, or one word; anything else is none.
                    src: match raw.src {
                        Some(serde_json::Value::Array(list)) => list
                            .iter()
                            .filter_map(|v| v.as_str())
                            .map(|v| v.trim().to_string())
                            .filter(|v| !v.is_empty())
                            .collect(),
                        Some(serde_json::Value::String(one)) if !one.trim().is_empty() => vec![one.trim().to_string()],
                        _ => Vec::new(),
                    },
                    before: nonempty(raw.before),
                    after: nonempty(raw.after),
                    // A label without a color that reads is shown gray.
                    label: nonempty(raw.label).map(|name| Label {
                        name,
                        color: raw.label_color.filter(|c| hex_color(c)).unwrap_or_else(|| "#94A3B8".to_string()),
                    }),
                })
            })
            .collect(),
    )
}

/// The ids Kova passes on: letters, digits, `-` and `_`, 64 at most, and no
/// leading dash, so the CLI can never read one as an option.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('-')
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Epoch seconds of an ISO 8601 date with its offset
/// (`2026-11-05T16:22:48+01:00`, `...Z`). `None` when it is anything else:
/// a date without an offset would be read in some zone, so it is refused.
pub fn epoch_of(ts: &str) -> Option<u64> {
    let b = ts.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { ts.get(from..to)?.parse().ok() };
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    // Fractions of a second are dropped; the offset follows them.
    let mut rest = &ts[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.chars().take_while(char::is_ascii_digit).count();
        rest = &frac[digits..];
    }
    // Bytes, not string slices: a byte index inside a multibyte character
    // would panic, and the date comes from outside.
    let r = rest.as_bytes();
    let offset = match r {
        b"Z" => 0,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] if [h1, h2, m1, m2].iter().all(|b| b.is_ascii_digit()) => {
            let h = i64::from(h1 - b'0') * 10 + i64::from(h2 - b'0');
            let m = i64::from(m1 - b'0') * 10 + i64::from(m2 - b'0');
            if h > 23 || m > 59 {
                return None;
            }
            let sign = if *sign == b'-' { -1 } else { 1 };
            sign * (h * 3600 + m * 60)
        }
        _ => return None,
    };
    // Days from the civil date (Howard Hinnant's algorithm), no zone lookup.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset;
    u64::try_from(secs).ok()
}

// ---------------------------------------------------------------
// Commands
// ---------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Commands {
    pub list: Vec<String>,
    pub act: Vec<String>,
    pub close: Vec<String>,
    /// `attn snooze`; empty: the program of `close` with `snooze`.
    pub snooze: Vec<String>,
    /// `attn explain`; empty: the program of `close` with `explain`.
    pub explain: Vec<String>,
}

static COMMANDS: RwLock<Commands> = RwLock::new(Commands {
    list: Vec::new(),
    act: Vec::new(),
    close: Vec::new(),
    snooze: Vec::new(),
    explain: Vec::new(),
});

/// The argv of a verb the config may leave out: as given, or the program of
/// `close` followed by `verb`.
fn or_close_bin(given: &[String], close: &[String], verb: &str) -> Vec<String> {
    if !given.is_empty() {
        return given.to_vec();
    }
    close.first().map(|bin| vec![bin.clone(), verb.to_string()]).unwrap_or_default()
}

/// Adopt the configured commands. An empty `list` turns the queue off: the
/// Cockpit shows the live needs only and nothing is spawned. Called at startup and on
/// `reload-config`.
pub fn set_commands(config: &Commands) {
    let next = Commands {
        list: config.list.clone(),
        act: config.act.clone(),
        close: config.close.clone(),
        snooze: or_close_bin(&config.snooze, &config.close, "snooze"),
        explain: or_close_bin(&config.explain, &config.close, "explain"),
    };
    if *COMMANDS.read() == next {
        return;
    }
    *COMMANDS.write() = next;
    // What was read, and the clicks on it, belong to the old command; a read
    // still running under it is stale once `CHANGED` moved.
    *CACHE.write() = None;
    *OUTCOMES.lock() = None;
    *INSTRUCTIONS.lock() = None;
    CHANGED.fetch_add(1, Ordering::Relaxed);
    FILLED_AT.store(0, Ordering::Relaxed);
}

/// Whether the queue is on.
pub fn configured() -> bool {
    !COMMANDS.read().list.is_empty()
}

/// Whether the buttons that need `act_command` / `close_command` can run.
pub fn can_act() -> bool {
    !COMMANDS.read().act.is_empty()
}

pub fn can_close() -> bool {
    !COMMANDS.read().close.is_empty()
}

pub fn can_snooze() -> bool {
    !COMMANDS.read().snooze.is_empty()
}

static CACHE: RwLock<Option<Arc<Vec<Entry>>>> = RwLock::new(None);
static FILLED_AT: AtomicU64 = AtomicU64::new(0);
static REFRESHING: AtomicBool = AtomicBool::new(false);
/// Bumped by every click that changed the queue, so a read that began before
/// it cannot pass for the new state.
static CHANGED: AtomicU64 = AtomicU64::new(0);
static READ_AT: AtomicU64 = AtomicU64::new(0);

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Clears `REFRESHING` however the read thread ends, a panic included, and
/// marks the read time either way: a read that keeps failing is retried
/// every `MAX_AGE_SECS`, not on every tick.
struct RefreshDone;

impl Drop for RefreshDone {
    fn drop(&mut self) {
        FILLED_AT.store(now_secs(), Ordering::Relaxed);
        REFRESHING.store(false, Ordering::Release);
    }
}

/// The queue as last read, and a refresh in the background once it aged out.
/// Returns at once: `None` while the feature is off or before the first read
/// lands. A read that fails keeps the previous list (empty when there was
/// none), and logs why.
pub fn snapshot() -> Option<Arc<Vec<Entry>>> {
    if !configured() {
        return None;
    }
    let age = now_secs().saturating_sub(FILLED_AT.load(Ordering::Relaxed));
    let due = age >= MAX_AGE_SECS || READ_AT.load(Ordering::Relaxed) < CHANGED.load(Ordering::Relaxed);
    if due && !REFRESHING.swap(true, Ordering::AcqRel) {
        std::thread::spawn(|| {
            let _done = RefreshDone;
            let generation = CHANGED.load(Ordering::Relaxed);
            let argv = COMMANDS.read().list.clone();
            // A failed read keeps what the last good one said: an empty list
            // would also forget which rows were sent or closed from here.
            match read(&argv) {
                Some(entries) => *CACHE.write() = Some(Arc::new(entries)),
                None => {
                    let mut cache = CACHE.write();
                    if cache.is_none() {
                        *cache = Some(Arc::new(Vec::new()));
                    }
                }
            }
            READ_AT.store(generation, Ordering::Relaxed);
        });
    }
    CACHE.read().clone()
}

/// Force the next `snapshot()` to re-read.
pub fn refresh() {
    CHANGED.fetch_add(1, Ordering::Relaxed);
    FILLED_AT.store(0, Ordering::Relaxed);
}

/// Run the list command. `None`, logged, for a missing binary, a failure or
/// an unreadable answer: the caller keeps what it had.
fn read(argv: &[String]) -> Option<Vec<Entry>> {
    let (bin, args) = argv.split_first()?;
    if !Path::new(bin).is_file() {
        log::warn!("attention: {bin} is not a file");
        return None;
    }
    let out = match crate::exec::run_with_deadline(std::process::Command::new(bin).args(args), LIST_DEADLINE) {
        Ok(out) => out,
        Err(e) => {
            log::warn!("attention: {e}");
            return None;
        }
    };
    if !out.status.success() {
        log::warn!("attention: {bin} exited with {}", out.status);
        return None;
    }
    let entries = parse(&String::from_utf8_lossy(&out.stdout));
    if entries.is_none() {
        log::warn!("attention: cannot read the queue");
    }
    entries
}

// ---------------------------------------------------------------
// Clicks
// ---------------------------------------------------------------

/// What a row's button asks the CLI to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Verb {
    /// A draft approved as it is: `act_command <id> Valider`.
    Validate,
    /// A draft the user rewrote: `act_command <id> Corriger --text=<text>`.
    Correct,
    /// The Cockpit's check: `close_command <id> --reason done`.
    Resolve,
    /// The Cockpit's x, not relevant: `close_command <id> --reason dismissed`;
    /// `Refuser` adds its reason as `--note=<text>`.
    Dismiss,
    /// Out of the list until a date: `snooze_command <id> --until=<ISO>`.
    Snooze,
    /// A free instruction typed on an opened Cockpit card:
    /// `act_command <id> instruct --text=<text>`.
    Instruct,
    /// No go on an action plan, which is relevant but turned down:
    /// `close_command <id> --reason nogo`. Not the x (`Dismiss`).
    NoGo,
}

impl Verb {
    /// The word the button shows, and the one `act_command` receives.
    pub fn label(self) -> &'static str {
        match self {
            Verb::Validate => "Valider",
            Verb::Correct => "Corriger",
            Verb::Resolve => "C'est fait",
            Verb::Dismiss => "Retirer",
            Verb::Snooze => "Plus tard",
            Verb::Instruct => "instruct",
            Verb::NoGo => "No go",
        }
    }
}

/// Where one entry's click stands, for its row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The command runs: the buttons wait.
    Running,
    /// `Valider` or a correction went through and reached its agent
    /// (`act_command … --exit-codes` exited 0): the card says so instead of
    /// offering it again.
    Sent,
    /// The go did not go to an agent: the command sends the message itself
    /// and says so on its first line (`CODE_SEND`), shown as it is.
    SentByCode(String),
    /// The click is recorded but its agent does not have it yet (exit 3): it
    /// will be handed over later. Not a failure; the card says so.
    Noted,
    /// The entry was closed: hidden until the next read drops it.
    Closed,
    /// The command failed: its words, until `until_ms`.
    Failed { words: String, until_ms: u64 },
}

static OUTCOMES: Mutex<Option<HashMap<String, Outcome>>> = Mutex::new(None);

/// Every entry's click state, failures that ran out dropped.
pub fn outcomes() -> HashMap<String, Outcome> {
    let mut guard = OUTCOMES.lock();
    let map = guard.get_or_insert_with(HashMap::new);
    let now = now_ms();
    map.retain(|_, o| !matches!(o, Outcome::Failed { until_ms, .. } if *until_ms <= now));
    map.clone()
}

/// A click refused before its command ran (an entry gone from the queue, no
/// command configured): the card says why instead of doing nothing. One
/// still running is left alone.
pub fn click_refused(id: &str, words: &str) {
    let mut guard = OUTCOMES.lock();
    let map = guard.get_or_insert_with(HashMap::new);
    if matches!(map.get(id), Some(Outcome::Running)) {
        return;
    }
    let words = words.strip_prefix("attention: ").unwrap_or(words).to_string();
    map.insert(id.to_string(), Outcome::Failed { words, until_ms: now_ms() + FAILED_MS });
}

fn set_outcome(id: &str, outcome: Outcome) {
    OUTCOMES.lock().get_or_insert_with(HashMap::new).insert(id.to_string(), outcome);
}

/// The last instruction typed on an entry's card, and where its command
/// stands (`Running`, `Sent` or `Failed`). Kept apart from `OUTCOMES`: an
/// instruction never reads as a sent draft, and never blocks the draft's own
/// buttons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instructed {
    pub text: String,
    pub outcome: Outcome,
}

static INSTRUCTIONS: Mutex<Option<HashMap<String, Instructed>>> = Mutex::new(None);

/// Every entry's last instruction.
pub fn instructions() -> HashMap<String, Instructed> {
    INSTRUCTIONS.lock().get_or_insert_with(HashMap::new).clone()
}

/// An instruction refused before it ran (an entry gone, no command): the
/// card says so with its text, since the page already cleared the field. One
/// still running is left alone.
pub fn instruction_refused(id: &str, text: &str, words: &str) {
    if instructions().get(id).is_some_and(|i| i.outcome == Outcome::Running) {
        return;
    }
    set_instructed(id, &clean_text(text), Outcome::Failed { words: words.to_string(), until_ms: 0 });
}

fn set_instructed(id: &str, text: &str, outcome: Outcome) {
    INSTRUCTIONS
        .lock()
        .get_or_insert_with(HashMap::new)
        .insert(id.to_string(), Instructed { text: text.to_string(), outcome });
}

/// The argv a click runs: the configured command, the id, then fixed words.
/// `None` when the command for that verb is not configured or the id is not
/// one Kova passes on.
/// A correction carries its text as one `--text=...` element, never as a
/// word of its own, so the CLI can never read it as an option.
/// A snooze carries its date the same way, as `--until=<ISO 8601>`, and only
/// a date that reads as one.
#[cfg(test)]
fn verb_argv(act: &[String], close: &[String], id: &str, verb: Verb, text: Option<&str>) -> Option<(String, Vec<String>)> {
    verb_argv_with(act, close, &[], id, verb, text)
}

fn verb_argv_with(
    act: &[String],
    close: &[String],
    snooze: &[String],
    id: &str,
    verb: Verb,
    text: Option<&str>,
) -> Option<(String, Vec<String>)> {
    if !valid_id(id) {
        return None;
    }
    let acts = matches!(verb, Verb::Validate | Verb::Correct | Verb::Instruct);
    let base = match verb {
        Verb::Snooze => snooze,
        _ if acts => act,
        _ => close,
    };
    let (bin, words) = base.split_first()?;
    let mut args: Vec<String> = words.to_vec();
    // A click on an agent's card says how far it went (`run_click`).
    if acts {
        args.push(EXIT_CODES.to_string());
    }
    args.push(id.to_string());
    match verb {
        Verb::Validate => args.push(verb.label().to_string()),
        Verb::Correct => {
            let text = text.filter(|t| !t.trim().is_empty())?;
            args.extend([verb.label().to_string(), format!("--text={text}")]);
        }
        Verb::Instruct => {
            let text = text.filter(|t| !t.trim().is_empty())?;
            args.extend([verb.label().to_string(), format!("--text={text}")]);
        }
        Verb::Resolve => args.extend(["--reason".to_string(), "done".to_string()]),
        Verb::NoGo => args.extend(["--reason".to_string(), "nogo".to_string()]),
        Verb::Dismiss => {
            args.extend(["--reason".to_string(), "dismissed".to_string()]);
            // Refuser's reason, when one was typed: one argv element.
            if let Some(why) = text.map(str::trim).filter(|t| !t.is_empty()) {
                args.push(format!("--note={why}"));
            }
        }
        Verb::Snooze => {
            let until = text.map(str::trim).filter(|t| epoch_of(t).is_some())?;
            args.push(format!("--until={until}"));
        }
    }
    Some((bin.clone(), args))
}

/// Run a card's button on a background thread, with the text a correction
/// sends. Refused at once when the id is not in the last read, when a click
/// on it already runs, or when the command is not configured; the card then
/// keeps its buttons. The text is the user's own, typed on the Cockpit's
/// card: control characters but tab and newline are dropped, and it is
/// capped at `MAX_TEXT`.
pub fn perform_with(id: &str, verb: Verb, text: Option<&str>) -> Result<(), String> {
    let text = text.map(clean_text);
    let known = CACHE.read().as_ref().is_some_and(|entries| entries.iter().any(|e| e.id == id));
    if !known {
        return Err(format!("attention: no open entry {id:?}"));
    }
    let (bin, args) = {
        let commands = COMMANDS.read();
        verb_argv_with(&commands.act, &commands.close, &commands.snooze, id, verb, text.as_deref())
    }
    .ok_or_else(|| format!("attention: no command for {}", verb.label()))?;
    if verb == Verb::Instruct {
        let text = text.unwrap_or_default();
        {
            let mut guard = INSTRUCTIONS.lock();
            let map = guard.get_or_insert_with(HashMap::new);
            if map.get(id).is_some_and(|i| i.outcome == Outcome::Running) {
                return Err(format!("attention: an instruction to {id} already runs"));
            }
            map.insert(id.to_string(), Instructed { text: text.clone(), outcome: Outcome::Running });
        }
        let id = id.to_string();
        std::thread::spawn(move || {
            let outcome = match run_click(&bin, &args) {
                Ok(Delivery::Delivered | Delivery::ByCode(_)) => Outcome::Sent,
                Ok(Delivery::Noted) => Outcome::Noted,
                Err(words) => {
                    log::warn!("attention instruct {id}: {words}");
                    // Kept until the next instruction, not timed out: the
                    // field was cleared, the card keeps what did not go.
                    Outcome::Failed { words, until_ms: 0 }
                }
            };
            set_instructed(&id, &text, outcome);
            refresh();
        });
        return Ok(());
    }
    {
        let mut guard = OUTCOMES.lock();
        let map = guard.get_or_insert_with(HashMap::new);
        if matches!(map.get(id), Some(Outcome::Running)) {
            return Err(format!("attention: a click on {id} already runs"));
        }
        map.insert(id.to_string(), Outcome::Running);
    }
    let id = id.to_string();
    std::thread::spawn(move || {
        match run_click(&bin, &args) {
            Ok(delivery) => set_outcome(
                &id,
                match (verb, delivery) {
                    (Verb::Validate | Verb::Correct, Delivery::Delivered) => Outcome::Sent,
                    (Verb::Validate | Verb::Correct, Delivery::ByCode(line)) => Outcome::SentByCode(line),
                    (Verb::Validate | Verb::Correct, Delivery::Noted) => Outcome::Noted,
                    _ => Outcome::Closed,
                },
            ),
            Err(words) => {
                log::warn!("attention {} {id}: {words}", verb.label());
                set_outcome(&id, Outcome::Failed { words, until_ms: now_ms() + FAILED_MS });
            }
        }
        refresh();
    });
    Ok(())
}

/// The flag that makes `act_command` say how far a click went by its exit
/// code: 0 handed to the agent, 3 recorded and not handed over yet, any
/// other a failure with one line on stderr.
pub const EXIT_CODES: &str = "--exit-codes";
/// `act_command --exit-codes`: recorded, not handed to the agent yet.
const EXIT_NOTED: i32 = 3;

/// How far a click that did not fail went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivery {
    Delivered,
    Noted,
    /// The command sends the message itself: its own words.
    ByCode(String),
}

/// How `act_command` starts its first line when it sends a validated draft
/// itself instead of handing it to an agent (`envoi par le code (slack à
/// U123)`).
pub const CODE_SEND: &str = "envoi par le code";

/// The line a command that exited 0 printed, when it says it sends the
/// message itself: one line, 120 chars at most.
pub fn code_send_line(stdout: &str) -> Option<String> {
    let line = stdout.lines().next()?.trim();
    line.starts_with(CODE_SEND).then(|| line.chars().filter(|c| !c.is_control()).take(120).collect())
}

/// What a click's exit says: `code` is `None` when a signal ended it.
/// `exit_codes` is whether the command was asked for them; without the
/// flag, 3 is a failure like any other.
pub fn delivery_of(code: Option<i32>, exit_codes: bool) -> Option<Delivery> {
    match code {
        Some(0) => Some(Delivery::Delivered),
        Some(EXIT_NOTED) if exit_codes => Some(Delivery::Noted),
        _ => None,
    }
}

/// The environment variable that carries the click key to the CLI, and the
/// one that names who clicked.
pub const CLICK_KEY_ENV: &str = "ATTN_CLICK_KEY";
pub const CLICK_FROM_ENV: &str = "ATTN_CLICK_FROM";
/// This app, for `CLICK_FROM_ENV`.
pub const CLICK_FROM: &str = "kova";

static CLICK_KEY_FILE: RwLock<Option<std::path::PathBuf>> = RwLock::new(None);

/// `[attention] click_key_file`, set at load and on a config reload: the
/// path only. The key itself is read at each click and never kept.
pub fn set_click_key_file(path: Option<std::path::PathBuf>) {
    *CLICK_KEY_FILE.write() = path;
}

/// The click key in `path`: its one line, trimmed. `None` when the file is
/// not there, cannot be read, or does not hold one printable word of 16 to
/// 256 chars: the click then goes without a key, as before. Kova never
/// writes this file, and never logs the key: only that it could not be read.
pub fn click_key(path: &std::path::Path) -> Option<String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) => {
            log::warn!("attention: the click key file cannot be read ({})", e.kind());
            return None;
        }
    };
    let key = raw.trim();
    let sound = (16..=256).contains(&key.len()) && key.chars().all(|c| c.is_ascii_graphic());
    if !sound {
        log::warn!("attention: the click key file does not hold a key");
    }
    sound.then(|| key.to_string())
}

/// Run a click's argv: how far it went, or `Err` with the first line of its
/// stderr on a failure. Only the user's own clicks come here
/// (`perform_with`, called by the Cockpit's and the chat's pages, never by
/// the IPC nor the command line): the command gets the click key in its
/// environment, never in its argv, which any process can read.
fn run_click(bin: &str, args: &[String]) -> Result<Delivery, String> {
    let key = CLICK_KEY_FILE.read().clone().and_then(|path| click_key(&path));
    run_click_with(bin, args, key.as_deref())
}

fn run_click_with(bin: &str, args: &[String], key: Option<&str>) -> Result<Delivery, String> {
    let exit_codes = args.iter().any(|a| a == EXIT_CODES);
    let mut command = std::process::Command::new(bin);
    command.args(args);
    // Never inherited from Kova's own environment: only a key read now.
    command.env_remove(CLICK_KEY_ENV).env_remove(CLICK_FROM_ENV);
    if let Some(key) = key {
        command.env(CLICK_KEY_ENV, key).env(CLICK_FROM_ENV, CLICK_FROM);
    }
    crate::exec::run_with_deadline(&mut command, ACT_DEADLINE).and_then(|out| {
        if let Some(delivery) = delivery_of(out.status.code(), exit_codes) {
            Ok(match (delivery, code_send_line(&String::from_utf8_lossy(&out.stdout))) {
                (Delivery::Delivered, Some(line)) => Delivery::ByCode(line),
                (delivery, _) => delivery,
            })
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            let line: String = err.lines().next().unwrap_or("").trim().chars().take(80).collect();
            Err(if line.is_empty() { format!("{bin} exited with {}", out.status) } else { line })
        }
    })
}

/// How long `attn explain` may take: a model writes the detail.
const EXPLAIN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(150);
/// The ids an explain runs for: never two at once for one entry.
static EXPLAINING: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Whether an explain runs for this entry.
pub fn explaining(id: &str) -> bool {
    EXPLAINING.lock().iter().any(|i| i == id)
}

/// Ask the CLI to write an entry's detail, in the background
/// (`explain_command <id>`); the next read brings it. False when it cannot
/// run: no command, an id not in the last read, one already running.
pub fn explain(id: &str) -> bool {
    let known = CACHE.read().as_ref().is_some_and(|entries| entries.iter().any(|e| e.id == id));
    let argv = COMMANDS.read().explain.clone();
    let Some((bin, words)) = argv.split_first() else { return false };
    if !known || !valid_id(id) {
        return false;
    }
    {
        let mut running = EXPLAINING.lock();
        if running.iter().any(|i| i == id) {
            return false;
        }
        running.push(id.to_string());
    }
    let (bin, mut args, id) = (bin.clone(), words.to_vec(), id.to_string());
    args.push(id.clone());
    std::thread::spawn(move || {
        match crate::exec::run_with_deadline(std::process::Command::new(&bin).args(&args), EXPLAIN_DEADLINE) {
            Ok(out) if out.status.success() => {}
            Ok(out) => log::warn!("attention explain {id}: {bin} exited with {}", out.status),
            Err(e) => log::warn!("attention explain {id}: {e}"),
        }
        EXPLAINING.lock().retain(|i| i != &id);
        refresh();
    });
    true
}

/// The most a correction carries: a long email, with room to spare.
pub const MAX_TEXT: usize = 16 * 1024;

/// A correction as it goes into the argv: no control character but tab and
/// newline, `MAX_TEXT` bytes at most (cut on a character boundary).
pub fn clean_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_TEXT));
    for c in text.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t') {
        if out.len() + c.len_utf8() > MAX_TEXT {
            break;
        }
        out.push(c);
    }
    out
}

/// Forget the click state of entries the queue no longer holds: a closed
/// entry left, a sent one was closed by its receiver. A click still running
/// keeps its state.
pub fn prune(entries: &[Entry]) {
    if let Some(map) = OUTCOMES.lock().as_mut() {
        map.retain(|id, o| matches!(o, Outcome::Running) || entries.iter().any(|e| &e.id == id));
    }
    if let Some(map) = INSTRUCTIONS.lock().as_mut() {
        map.retain(|id, i| i.outcome == Outcome::Running || entries.iter().any(|e| &e.id == id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/attention/list.json");

    #[test]
    fn a_label_reads_with_its_color_and_a_color_that_does_not_read_is_gray() {
        let list = parse(r##"[{"id":"a-1","level":"now","title":"t","label":"QA","label_color":"#F59E0B"},
            {"id":"a-2","level":"now","title":"t","label":"Dev","label_color":"red;x"},{"id":"a-3","level":"now","title":"t","label":" "}]"##)
        .unwrap();
        assert_eq!(list[0].label, Some(Label { name: "QA".into(), color: "#F59E0B".into() }));
        assert_eq!(list[1].label.as_ref().unwrap().color, "#94A3B8");
        assert_eq!(list[2].label, None);
    }

    #[test]
    fn the_fixture_parses_and_a_bad_entry_is_skipped_alone() {
        let entries = parse(FIXTURE).expect("an array");
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        // An item without an id, `a-bad001` (an unknown level) and `-a-dash`
        // (a leading dash) are left out, nothing else is.
        assert_eq!(ids, ["a-100001", "a-100002", "a-100003", "a-100004", "a-100005"]);
        let first = &entries[0];
        assert_eq!(first.level, Level::Now);
        assert_eq!(first.kind, "decision");
        assert_eq!(first.needs.as_deref(), Some("Pick one of the two plans"));
        assert_eq!(first.action.as_deref(), Some("https://example.com/plan"));
        assert_eq!(first.since, 1_793_892_168);
        // An empty `needs` is no needs, and an absent topic is none.
        assert_eq!(entries[1].needs, None);
        assert_eq!(entries[4].topic, None);
        // A date that does not read sorts as the oldest, it is not dropped.
        assert_eq!(entries[4].since, 0);
        assert!(parse("{}").is_none());
        assert_eq!(parse("[]"), Some(Vec::new()));
    }

    #[test]
    fn dates_are_read_with_their_offset() {
        assert_eq!(epoch_of("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_of("2026-11-05T16:22:48+01:00"), Some(1_793_892_168));
        assert_eq!(epoch_of("2026-11-05T15:22:48Z"), Some(1_793_892_168));
        assert_eq!(epoch_of("2026-11-05T15:22:48.250Z"), Some(1_793_892_168));
        assert_eq!(epoch_of("2026-11-05T10:22:48-05:00"), Some(1_793_892_168));
        // No offset, a broken date: refused rather than guessed.
        assert_eq!(epoch_of("2026-11-05T16:22:48"), None);
        assert_eq!(epoch_of("2026-13-05T16:22:48Z"), None);
        assert_eq!(epoch_of("yesterday"), None);
        // Multibyte characters where the offset goes: refused, never a panic.
        assert_eq!(epoch_of("2026-11-05T16:22:48+1é00"), None);
        assert_eq!(epoch_of("2026-11-05T16:22:48+€xx"), None);
        assert_eq!(epoch_of("2026-11-05T16:22:48é+01:00"), None);
        assert_eq!(epoch_of("2026-11-05T16:22:48+99:00"), None);
    }

    #[test]
    fn ids_never_carry_an_option_or_a_shell_word() {
        assert!(valid_id("a-36d0b2"));
        for bad in ["", "-rf", "--note", "a b", "a;rm", "a/b", "a\nb", &"x".repeat(65)] {
            assert!(!valid_id(bad), "accepted {bad:?}");
        }
    }

    #[test]
    fn a_click_runs_the_configured_command_with_the_id_and_fixed_words() {
        let act: Vec<String> = ["/bin/attn", "act"].iter().map(|s| s.to_string()).collect();
        let close: Vec<String> = ["/bin/attn", "close", "--by", "me"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            verb_argv(&act, &close, "a-1", Verb::Resolve, None),
            Some((
                "/bin/attn".to_string(),
                vec!["close".into(), "--by".into(), "me".into(), "a-1".into(), "--reason".into(), "done".into()]
            ))
        );
        assert_eq!(
            verb_argv(&act, &close, "a-1", Verb::Dismiss, None).map(|(_, a)| a[4..].to_vec()),
            Some(vec!["--reason".to_string(), "dismissed".to_string()])
        );
        assert_eq!(verb_argv(&act, &close, "a-1", Verb::Resolve, None).map(|(_, a)| a[5].clone()), Some("done".into()));
        // Refuser's reason rides along as one `--note=` element; blank adds nothing.
        assert_eq!(
            verb_argv(&act, &close, "a-1", Verb::Dismiss, Some(" --by evil\npas maintenant ")).map(|(_, a)| a[4..].to_vec()),
            Some(vec!["--reason".to_string(), "dismissed".to_string(), "--note=--by evil\npas maintenant".to_string()])
        );
        assert_eq!(verb_argv(&act, &close, "a-1", Verb::Dismiss, Some("  ")).map(|(_, a)| a.len()), Some(6));
        let snooze = or_close_bin(&[], &close, "snooze");
        assert_eq!(snooze, ["/bin/attn", "snooze"]);
        assert_eq!(
            verb_argv_with(&act, &close, &snooze, "a-1", Verb::Snooze, Some("2026-11-07T09:00:00+01:00")),
            Some(("/bin/attn".to_string(), vec!["snooze".into(), "a-1".into(), "--until=2026-11-07T09:00:00+01:00".into()]))
        );
        assert_eq!(verb_argv_with(&act, &close, &snooze, "a-1", Verb::Snooze, Some("--by x")), None, "only a date");
        assert_eq!(or_close_bin(&["/x/explain".into()], &close, "explain"), ["/x/explain"]);
        assert!(or_close_bin(&[], &[], "explain").is_empty());
        // No close command: C'est fait has nothing to run.
        assert_eq!(verb_argv(&act, &[], "a-1", Verb::Resolve, None), None);
        // An id that could be read as an option never reaches the argv.
        assert_eq!(verb_argv(&act, &close, "--help", Verb::Validate, None), None);
    }

    #[test]
    fn an_entry_carries_its_failure_its_proof_and_whether_it_is_a_plan() {
        let entries = parse(
            r#"[{"id":"a-1","level":"point","kind":"decision","from":"veille-actions","title":"Leave the MDM","body":"Plan : 1) write to support 2) remove the profile"},
                {"id":"a-2","level":"now","kind":"decision","from":"pm","title":"Pick","body":"Plan : 1) x","failed":" Slack refused ","proof":""},
                {"id":"a-3","level":"now","kind":"info","from":"veille-actions","title":"Digest","body":"x","proof":"commit 53fdad9"},
                {"id":"a-4","level":"now","kind":"decision","from":"veille-actions","title":"No body"}]"#,
        )
        .unwrap();
        assert_eq!(entries.iter().map(|e| e.plan).collect::<Vec<_>>(), [true, false, false, false], "a decision of the plans' producer, with its plan");
        assert_eq!(entries[1].failed.as_deref(), Some("Slack refused"));
        assert_eq!((entries[1].proof.as_deref(), entries[2].proof.as_deref()), (None, Some("commit 53fdad9")));
        // No go closes with its own reason, through the close command.
        let close: Vec<String> = ["/bin/attn", "close", "--by", "robin"].iter().map(|s| s.to_string()).collect();
        assert_eq!(verb_argv(&[], &close, "a-1", Verb::NoGo, None).map(|(_, a)| a), Some(vec!["close".into(), "--by".into(), "robin".into(), "a-1".into(), "--reason".into(), "nogo".into()]));
    }

    #[test]
    fn a_click_carries_its_key_in_the_environment_never_in_the_argv() {
        let dir = std::env::temp_dir().join(format!("kova-click-key-{}", crate::ask::new_question_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("key");
        let key = "0123456789abcdef0123456789abcdef";
        std::fs::write(&file, format!("{key}\n")).unwrap();
        assert_eq!(click_key(&file).as_deref(), Some(key), "one line, trimmed");
        std::fs::write(&file, "short\n").unwrap();
        assert_eq!(click_key(&file), None);
        std::fs::write(&file, "two words that are long enough\n").unwrap();
        assert_eq!(click_key(&file), None, "not one word");
        assert_eq!(click_key(&dir.join("none")), None, "no file, no key, and none is made");
        assert!(!dir.join("none").exists());
        // The command sees the key and who clicked in its environment; its
        // argv is what it was. (The script writes what it got to a file.)
        let seen = dir.join("seen");
        let script = dir.join("act.sh");
        let body = format!(
            "#!/bin/sh\nprintf '%s|%s|%s' \"${}\" \"${}\" \"$*\" > '{}'\necho 'refused by the queue' >&2\nexit \"$1\"\n",
            CLICK_KEY_ENV,
            CLICK_FROM_ENV,
            seen.display()
        );
        std::fs::write(&script, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let bin = script.to_string_lossy().into_owned();
        let args = |code: &str| vec![code.to_string(), "a-1".to_string(), "Valider".to_string()];
        assert_eq!(run_click_with(&bin, &args("0"), Some(key)), Ok(Delivery::Delivered));
        assert_eq!(std::fs::read_to_string(&seen).unwrap(), format!("{key}|kova|0 a-1 Valider"), "in the environment, not in the argv");
        // A failure's words never hold the key.
        let failed = run_click_with(&bin, &args("1"), Some(key)).unwrap_err();
        assert_eq!(failed, "refused by the queue");
        // Without a key (no file, or one that does not read): the click goes
        // as before, and a key or an origin in Kova's own environment is not
        // passed on.
        assert_eq!(run_click_with(&bin, &args("0"), None), Ok(Delivery::Delivered));
        assert_eq!(std::fs::read_to_string(&seen).unwrap(), "||0 a-1 Valider");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The key goes with the user's own click only: `perform_with` is called
    /// by the Cockpit's and the chat's pages and by nothing else (no IPC
    /// command, no `kova` subcommand, no rendering), `run_click` by
    /// `perform_with` only, and no test runs with a key file set.
    #[test]
    fn only_a_click_on_a_page_can_carry_the_key() {
        fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    sources(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        sources(&root, &mut files);
        let rel = |p: &std::path::Path| p.strip_prefix(&root).unwrap().to_string_lossy().into_owned();
        // The code before the tests module, comments dropped.
        let code = |p: &std::path::Path| -> String {
            let text = std::fs::read_to_string(p).unwrap();
            let body = text.split("\n#[cfg(test)]\nmod tests").next().unwrap_or("").to_string();
            body.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
        };
        let mut calling: Vec<String> = files.iter().filter(|p| code(p).contains("perform_with(")).map(|p| rel(p)).collect();
        calling.sort();
        assert_eq!(calling, ["attention.rs", "window/chat_view.rs", "window/cockpit_view.rs"], "who can run a click");
        for p in &files {
            let (name, body) = (rel(p), code(p));
            if name != "attention.rs" {
                assert!(!body.contains("run_click") && !body.contains("click_key(") && !body.contains("CLICK_KEY_ENV"), "{name} must not touch the click key");
            }
        }
        // In this file: the key is read in one place, used in one place.
        let own = code(&root.join("attention.rs"));
        assert_eq!(own.matches("click_key(&path)").count(), 1);
        assert_eq!(own.matches("run_click(&bin, &args)").count(), 2, "the instruction and the other verbs, both inside perform_with");
        let perform = own.split("pub fn perform_with(").nth(1).unwrap().split("\nfn run_click(").next().unwrap();
        assert_eq!(perform.matches("run_click(&bin, &args)").count(), 2);
        // No test runs with a key file: the path is only set from the config.
        assert!(CLICK_KEY_FILE.read().is_none());
    }

    #[test]
    fn a_click_s_exit_code_says_delivered_noted_or_failed() {
        assert_eq!(delivery_of(Some(0), true), Some(Delivery::Delivered));
        assert_eq!(delivery_of(Some(3), true), Some(Delivery::Noted), "recorded, not handed over yet: not a failure");
        assert_eq!(delivery_of(Some(1), true), None);
        assert_eq!(delivery_of(Some(2), true), None);
        assert_eq!(delivery_of(None, true), None, "killed by a signal");
        // The command sends the message itself and says so: its line, as it is.
        assert_eq!(code_send_line("envoi par le code (slack à U0BJ301TJNM)\nmore\n").as_deref(), Some("envoi par le code (slack à U0BJ301TJNM)"));
        assert_eq!(code_send_line("remis à claap-agent\n"), None);
        assert_eq!(code_send_line(""), None);
        assert_eq!(delivery_of(Some(3), false), None, "a command not asked for exit codes: 3 is a failure");
        // Only a click on an agent's card asks for them; a close does not.
        let act: Vec<String> = ["/bin/attn", "act"].iter().map(|s| s.to_string()).collect();
        let close: Vec<String> = ["/bin/attn", "close"].iter().map(|s| s.to_string()).collect();
        assert!(verb_argv(&act, &close, "a-1", Verb::Validate, None).unwrap().1.contains(&EXIT_CODES.to_string()));
        assert!(!verb_argv(&act, &close, "a-1", Verb::Resolve, None).unwrap().1.contains(&EXIT_CODES.to_string()));
        // A click refused before it ran shows on the card; one that runs is left alone.
        click_refused("a-refused", "attention: no open entry \"a-refused\"");
        assert!(matches!(outcomes().get("a-refused"), Some(Outcome::Failed { words, .. }) if words == "no open entry \"a-refused\""));
        set_outcome("a-running", Outcome::Running);
        click_refused("a-running", "attention: a click on a-running already runs");
        assert_eq!(outcomes().get("a-running"), Some(&Outcome::Running));
    }

    #[test]
    fn a_draft_is_validated_or_corrected_and_the_text_is_one_argv_element() {
        let act: Vec<String> = ["/bin/attn", "act"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            verb_argv(&act, &[], "a-1", Verb::Validate, None).map(|(_, a)| a),
            Some(vec!["act".into(), "--exit-codes".into(), "a-1".into(), "Valider".into()])
        );
        // A text that looks like an option stays inside its `--text=` element.
        assert_eq!(
            verb_argv(&act, &[], "a-1", Verb::Correct, Some("--by evil\nBonjour")).map(|(_, a)| a),
            Some(vec!["act".into(), "--exit-codes".into(), "a-1".into(), "Corriger".into(), "--text=--by evil\nBonjour".into()])
        );
        // A correction with nothing in it is not one.
        assert_eq!(verb_argv(&act, &[], "a-1", Verb::Correct, Some("  ")), None);
        assert_eq!(verb_argv(&act, &[], "a-1", Verb::Correct, None), None);
                assert_eq!(clean_text("a\x1b[201~b\u{7}\nc\td"), "a[201~b\nc\td");
        assert_eq!(clean_text(&"é".repeat(MAX_TEXT)).len(), MAX_TEXT);
    }

    #[test]
    fn an_instruction_is_act_id_instruct_with_its_text_as_one_argv_element() {
        let act: Vec<String> = ["/bin/attn", "act"].iter().map(|s| s.to_string()).collect();
        // Quotes, a newline and an option-like start stay inside `--text=`:
        // no shell runs it, so nothing needs quoting.
        let text = "Envoie les relevés \"octobre\" à l'expert\n--by evil; rm -rf ~";
        assert_eq!(
            verb_argv(&act, &[], "a-1", Verb::Instruct, Some(text)),
            Some(("/bin/attn".to_string(), vec!["act".into(), "--exit-codes".into(), "a-1".into(), "instruct".into(), format!("--text={text}")]))
        );
        // Nothing typed, nothing sent.
        assert_eq!(verb_argv(&act, &[], "a-1", Verb::Instruct, Some(" \n ")), None);
        assert_eq!(verb_argv(&act, &[], "a-1", Verb::Instruct, None), None);
        // No act command, no instruction.
        assert_eq!(verb_argv(&[], &[], "a-1", Verb::Instruct, Some("x")), None);
    }

    #[test]
    fn scope_prio_and_draft_are_read_and_a_bad_one_is_none() {
        let entries = parse(FIXTURE).expect("an array");
        assert_eq!(entries[0].scope, Scope::Perso);
        assert_eq!(entries[0].prio, Some(1));
        let draft = entries[0].draft.as_ref().expect("a draft");
        assert_eq!((draft.to.as_str(), draft.channel.as_str()), ("The accountant", "Email"));
        assert_eq!(draft.subject.as_deref(), Some("October statements"));
        assert_eq!(draft.text, "Hello,\nThey follow this week.");
        // No scope is work; a priority out of range and a draft without text
        // are none.
        assert_eq!(entries[1].scope, Scope::Pro);
        assert_eq!(entries[1].prio, None);
        assert_eq!(entries[1].draft, None);
        assert_eq!(entries[2].prio, Some(3));
        let more = parse(r#"[{"id":"a-9","level":"point","title":"t","body":"line 1\nline 2","detail":" ","src":["s-1"," "]},
            {"id":"a-8","level":"point","title":"t","src":"https://x.y"}]"#)
        .unwrap();
        assert_eq!(more[0].body.as_deref(), Some("line 1\nline 2"));
        assert_eq!(more[0].detail, None);
        assert_eq!(more[0].src, ["s-1"]);
        assert_eq!(more[1].src, ["https://x.y"]);
    }

    #[test]
    fn a_sent_or_closed_row_keeps_its_state_until_the_queue_drops_it() {
        let entry = parse(FIXTURE).expect("an array").remove(0);
        set_outcome("a-100001", Outcome::Sent);
        set_outcome("a-gone", Outcome::Closed);
        set_outcome("a-busy", Outcome::Running);
        prune(std::slice::from_ref(&entry));
        let states = outcomes();
        assert_eq!(states.get("a-100001"), Some(&Outcome::Sent));
        assert_eq!(states.get("a-gone"), None, "the queue no longer holds it");
        assert_eq!(states.get("a-busy"), Some(&Outcome::Running), "a click in flight is kept");
        // A failure's words run out on their own.
        set_outcome("a-100001", Outcome::Failed { words: "boom".into(), until_ms: 1 });
        assert_eq!(outcomes().get("a-100001"), None);
    }

    #[test]
    fn the_feature_is_off_without_a_command() {
        set_commands(&Commands::default());
        assert!(!configured());
        assert!(snapshot().is_none());
        assert!(perform_with("a-100001", Verb::Resolve, None).is_err());
    }

    #[test]
    fn the_list_command_runs_with_the_words_the_config_names() {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/attention/list.json");
        let argv: Vec<String> = ["/bin/sh", "-c", &format!("cat '{fixture}'")].iter().map(|s| s.to_string()).collect();
        assert_eq!(read(&argv).map(|e| e.len()), Some(5));
        // A missing CLI or a failing one is no read at all, not an empty queue.
        let missing: Vec<String> = vec!["/nonexistent/attn".into()];
        assert_eq!(read(&missing), None);
        let failing: Vec<String> = ["/bin/sh", "-c", "exit 3"].iter().map(|s| s.to_string()).collect();
        assert_eq!(read(&failing), None);
    }
}
