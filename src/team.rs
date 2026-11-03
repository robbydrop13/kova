//! The agent roster the Agents view lists: the command named by
//! `[sidebar] roster_command` (nothing runs when the key is absent), read
//! like `routines::snapshot` (cached, refreshed on a background thread, never
//! on the tick) and reopened with its own argv.
//!
//! The CLI owns the names and the shape; this module only validates what it
//! would run. `reopen` accepts nothing but `team reopen <valid-name>`, and
//! the name is validated before it reaches a `Command`, so a roster entry
//! can never smuggle another argument or a shell. The path of the binary is
//! never hard-coded: it is the one the config names, in tracked files or
//! nowhere. See `docs/sidebar-spec.md`.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;

use serde::Deserialize;

/// How long a read stays fresh: the roster changes when a human opens or
/// closes a session, so a minute is plenty and cheap.
pub const MAX_AGE_SECS: u64 = 60;

/// Whether a roster entry has its session open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    #[default]
    Open,
    Closed,
}

/// One manager or dedicated session, as the CLI reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Entry {
    pub name: String,
    #[serde(default)]
    pub role: String,
    /// The Kova tab the manager lives in, when it has one.
    #[serde(default)]
    pub tab: Option<String>,
    /// The project a dedicated session works on.
    #[serde(default)]
    pub project: Option<String>,
    pub state: State,
    /// The argv that brings it back. Only `team reopen <name>` is ever run.
    #[serde(default)]
    pub reopen: Vec<String>,
}

/// The roster as last read. `available` is false when the CLI is missing or
/// its answer could not be read: the view then hides the Team sections
/// rather than claiming an empty team.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Team {
    #[serde(default)]
    pub managers: Vec<Entry>,
    #[serde(default)]
    pub dedicated: Vec<Entry>,
    #[serde(skip)]
    pub available: bool,
}

impl Team {
    /// The entry with this name, managers first.
    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.managers.iter().chain(self.dedicated.iter()).find(|e| e.name == name)
    }

    /// Parse the CLI's answer, for the fixture and the readers below. The
    /// `available` flag is what tells "no roster" from "an empty one".
    pub fn from_json(text: &str) -> Option<Team> {
        let mut team: Team = serde_json::from_str(text).ok()?;
        team.available = true;
        Some(team)
    }
}

/// The roster CLI, as argv (`[sidebar] roster_command`): the program and the
/// words that print the roster. Empty means no roster at all.
static COMMAND: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Adopt the configured roster command. An empty list turns the roster off:
/// both Team sections hide and no process is spawned. Called once at startup
/// and again on `reload-config`.
pub fn set_command(argv: &[String]) {
    if COMMAND.read().as_slice() == argv {
        return;
    }
    *COMMAND.write() = argv.to_vec();
    // What was read belongs to the old command, and the next tick must not
    // show it under the new one.
    *CACHE.write() = None;
    FILLED_AT.store(0, Ordering::Relaxed);
}

/// Whether a roster command is configured. When it is not, the view hides
/// the Team and Dedicated sections rather than claiming an empty team.
pub fn configured() -> bool {
    !COMMAND.read().is_empty()
}

/// The configured command, cloned for a background read.
fn command() -> Vec<String> {
    COMMAND.read().clone()
}

static CACHE: RwLock<Option<Arc<Team>>> = RwLock::new(None);
/// When the cache was last filled, epoch seconds. 0 means never.
static FILLED_AT: AtomicU64 = AtomicU64::new(0);
/// Bumped by every reopen so a read that began before it cannot pass for
/// the new state.
static CHANGED: AtomicU64 = AtomicU64::new(0);
static READ_AT: AtomicU64 = AtomicU64::new(0);
/// Set while a refresh thread runs, so a tick never starts a second one.
static REFRESHING: AtomicBool = AtomicBool::new(false);
/// The reopens in flight, by session name: a second click while the first one
/// runs is refused instead of starting a second `team reopen`.
static REOPENING: parking_lot::Mutex<Vec<String>> = parking_lot::Mutex::new(Vec::new());

/// How long a roster read may take: `team list --json` answers in
/// milliseconds on a working machine, and a hung one must not hold the refresh
/// flag for good.
const LIST_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);
/// How long a reopen may take: it starts a session, slower than a read but
/// still bounded.
const REOPEN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Clears `REFRESHING` however the read thread ends, a panic included: a flag
/// left set would keep every later tick from refreshing again.
struct RefreshDone;

impl Drop for RefreshDone {
    fn drop(&mut self) {
        REFRESHING.store(false, Ordering::Release);
    }
}

/// Removes a name from `REOPENING` when its thread ends, timeout or panic.
struct ReopenDone(String);

impl Drop for ReopenDone {
    fn drop(&mut self) {
        REOPENING.lock().retain(|name| name != &self.0);
    }
}

/// The roster as last read, and a refresh in the background when the snapshot
/// has aged out. Returns at once, and `None` until the first read lands.
pub fn snapshot() -> Option<Arc<Team>> {
    if !configured() {
        return None;
    }
    let age = now_secs().saturating_sub(FILLED_AT.load(Ordering::Relaxed));
    let due = age >= MAX_AGE_SECS || READ_AT.load(Ordering::Relaxed) < CHANGED.load(Ordering::Relaxed);
    if due && !REFRESHING.swap(true, Ordering::AcqRel) {
        std::thread::spawn(|| {
            let _done = RefreshDone;
            let generation = CHANGED.load(Ordering::Relaxed);
            let team = read(&command());
            *CACHE.write() = Some(Arc::new(team));
            FILLED_AT.store(now_secs(), Ordering::Relaxed);
            READ_AT.store(generation, Ordering::Relaxed);
        });
    }
    CACHE.read().clone()
}

/// Force the next `snapshot()` to re-read: a reopen just changed the roster.
pub fn refresh() {
    CHANGED.fetch_add(1, Ordering::Relaxed);
    FILLED_AT.store(0, Ordering::Relaxed);
}

/// One background reopen's outcome, waiting for the tick that owns the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReopenOutcome {
    pub name: String,
    /// The CLI's first stderr line when it failed.
    pub result: Result<(), String>,
}

static RESULTS: parking_lot::Mutex<Vec<ReopenOutcome>> = parking_lot::Mutex::new(Vec::new());

/// Everything the reopens finished since the last drain.
pub fn drain_results() -> Vec<ReopenOutcome> {
    std::mem::take(&mut *RESULTS.lock())
}

/// Run an entry's reopen on a background thread, then force a refresh and
/// leave the outcome in `RESULTS`. The argv is validated here, on the
/// caller's thread, so a roster entry that says anything but
/// `team reopen <valid-name>` never reaches a `Command`.
pub fn reopen(name: &str, argv: &[String]) -> Result<(), String> {
    if !valid_reopen(name, argv) {
        return Err(format!("refusing reopen argv: {argv:?}"));
    }
    let Some((bin, args)) = reopen_command(name, argv, &command()) else {
        return Err("no roster command configured".to_string());
    };
    let name = name.to_string();
    {
        let mut in_flight = REOPENING.lock();
        if in_flight.contains(&name) {
            return Err(format!("a reopen of {name} is already in flight"));
        }
        in_flight.push(name.clone());
    }
    std::thread::spawn(move || {
        let _done = ReopenDone(name.clone());
        let result = run(&bin, &args);
        if let Err(why) = &result {
            log::warn!("team reopen {name}: {why}");
        }
        RESULTS.lock().push(ReopenOutcome { name: name.clone(), result });
        // The CLI updates its state file before it exits: the next read sees
        // the session open, and the row stops saying `Opening…`.
        refresh();
    });
    Ok(())
}

/// What a validated reopen runs: the configured binary, then the entry's own
/// `reopen <name>` words. The entry never chooses the program, the program is
/// the one the config names; and it never chooses a word before `reopen`
/// either, `valid_reopen` having checked the three of them.
fn reopen_command(name: &str, argv: &[String], roster: &[String]) -> Option<(String, Vec<String>)> {
    let (bin, _) = roster.split_first()?;
    if !valid_reopen(name, argv) {
        return None;
    }
    Some((bin.clone(), argv[1..].to_vec()))
}

fn run(bin: &str, args: &[String]) -> Result<(), String> {
    let out = crate::exec::run_with_deadline(
        std::process::Command::new(bin).args(args),
        REOPEN_DEADLINE,
    )?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if err.trim().is_empty() {
        Err(format!("{bin} exited with {}", out.status))
    } else {
        Err(first_line(&err))
    }
}

/// The first stderr line, capped at 80 characters.
fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().chars().take(80).collect()
}

/// The names the CLI accepts: lowercase, digits, dashes, and a leading
/// letter or digit. Anything else is refused before it reaches a `Command`.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Whether this is exactly `team reopen <name>`, for that entry's name.
pub fn valid_reopen(name: &str, argv: &[String]) -> bool {
    argv.len() == 3 && argv[0] == "team" && argv[1] == "reopen" && argv[2] == name && valid_name(name)
}

/// Read the roster by running `argv` (the configured command). An empty argv,
/// or a binary that is not there, is an unavailable roster: the view hides
/// the Team sections rather than claiming an empty team.
fn read(argv: &[String]) -> Team {
    let Some((bin, args)) = argv.split_first() else {
        return Team::default();
    };
    let bin = Path::new(bin);
    if !bin.is_file() {
        return Team::default();
    }
    let out = match crate::exec::run_with_deadline(
        std::process::Command::new(bin).args(args),
        LIST_DEADLINE,
    ) {
        Ok(out) => out,
        Err(e) => {
            log::warn!("roster: {e}");
            return Team::default();
        }
    };
    if !out.status.success() {
        log::warn!("roster: {} exited with {}", bin.display(), out.status);
        return Team::default();
    }
    match Team::from_json(&String::from_utf8_lossy(&out.stdout)) {
        Some(team) => team,
        None => {
            log::warn!("roster: cannot read the roster");
            Team::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Team {
        Team::from_json(include_str!("../tests/fixtures/team/list.json")).expect("the roster fixture")
    }

    #[test]
    fn the_fixture_parses_into_managers_and_dedicated_sessions() {
        let team = fixture();
        assert!(team.available);
        let names: Vec<&str> = team.managers.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["alpha-manager", "beta-coach"]);
        assert_eq!(team.managers[0].state, State::Open);
        assert_eq!(team.managers[1].state, State::Closed);
        assert_eq!(team.managers[0].tab.as_deref(), Some("Alpha"));
        let dedicated: Vec<&str> = team.dedicated.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(dedicated, ["gamma-project", "delta-project"]);
        assert_eq!(team.dedicated[0].project.as_deref(), Some("gamma"));
        assert_eq!(team.dedicated[1].state, State::Closed);
        // An entry is found across both lists.
        assert_eq!(team.entry("delta-project").map(|e| e.role.as_str()), Some("delta"));
        assert_eq!(team.entry("nobody").map(|e| e.name.as_str()), None);
    }

    #[test]
    fn no_roster_command_hides_the_sections() {
        set_command(&[]);
        assert!(!configured());
        // Nothing ran and there is nothing to show, which is what hides both
        // Team sections; no process is spawned either.
        assert!(snapshot().is_none());
    }

    #[test]
    fn a_missing_cli_is_an_unavailable_roster_not_an_empty_team() {
        assert!(!read(&[]).available);
        let missing: Vec<String> = ["/nonexistent/team", "list", "--json"].iter().map(|s| s.to_string()).collect();
        let team = read(&missing);
        assert!(!team.available);
        assert!(team.managers.is_empty());
    }

    #[test]
    fn the_roster_command_runs_with_the_words_the_config_names() {
        // `/bin/sh` stands in for the CLI: what matters is that the words
        // after the program reach it, so `list --json` is the command's own.
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/team/list.json");
        let argv: Vec<String> = ["/bin/sh", "-c", &format!("cat {fixture}")].iter().map(|s| s.to_string()).collect();
        let team = read(&argv);
        assert!(team.available);
        assert_eq!(team.entry("alpha-manager").map(|e| e.name.as_str()), Some("alpha-manager"));
    }

    #[test]
    fn reopen_only_accepts_team_reopen_and_a_valid_name() {
        let ok: Vec<String> = ["team", "reopen", "alpha-manager"].iter().map(|s| s.to_string()).collect();
        assert!(valid_reopen("alpha-manager", &ok));

        let cases = [
            vec!["bash", "reopen", "alpha-manager"],
            vec!["team", "open", "alpha-manager"],
            vec!["team", "reopen", "alpha-manager", "--force"],
            vec!["team", "reopen", "Alpha-Manager"],
            vec!["team", "reopen", "alpha manager"],
            vec!["team", "reopen", "alpha;rm -rf /"],
            vec!["team", "reopen", "-alpha"],
            vec!["team", "reopen", ""],
            vec!["team", "reopen"],
        ];
        for case in cases {
            let argv: Vec<String> = case.iter().map(|s| s.to_string()).collect();
            assert!(!valid_reopen("alpha-manager", &argv), "accepted {case:?}");
        }
        // The name must be the entry's own: an argv can never reopen another.
        assert!(!valid_reopen("beta-coach", &ok));
    }

    #[test]
    fn reopen_runs_the_configured_binary_and_only_the_entrys_own_words() {
        let argv: Vec<String> = ["team", "reopen", "alpha-manager"].iter().map(|s| s.to_string()).collect();
        let roster: Vec<String> = ["/usr/local/bin/team", "list", "--json"].iter().map(|s| s.to_string()).collect();
        let Some((bin, args)) = reopen_command("alpha-manager", &argv, &roster) else {
            panic!("a valid reopen has a command");
        };
        assert_eq!(bin, "/usr/local/bin/team");
        assert_eq!(args, ["reopen", "alpha-manager"]);
        // Without a configured command there is no binary to run.
        assert!(reopen_command("alpha-manager", &argv, &[]).is_none());
        // The entry's name is still its own: another entry's argv is refused.
        let other: Vec<String> = ["team", "reopen", "beta-coach"].iter().map(|s| s.to_string()).collect();
        assert!(reopen_command("alpha-manager", &other, &roster).is_none());
    }

    #[test]
    fn reopen_refuses_a_bad_argv_before_spawning_anything() {
        let bad: Vec<String> = ["sh", "-c", "true"].iter().map(|s| s.to_string()).collect();
        assert!(reopen("alpha-manager", &bad).is_err());
    }
}
