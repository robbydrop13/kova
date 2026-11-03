//! The scheduled local routines, read for the sidebar's Routines view.
//!
//! `~/.claude/routines/routinectl list --json` is the source of truth: one
//! object per routine, with its schedule, next run, last run and whether
//! launchd has it loaded, running or paused. Everything that groups or
//! phrases those numbers (`group`, `next_words`, `in_words`, `ago_words`) is
//! pure, with the clock passed in, so the words are testable.
//!
//! Routines used to be reassembled from three places that drift apart in
//! silence: the plist in `~/Library/LaunchAgents`, the prompt under
//! `~/.claude/routines/prompts`, and launchd itself, which knows nothing of
//! a plist until it is loaded. That reader stays as `read_from_disk`: it runs
//! instead, and is logged once, when routinectl is missing or prints
//! something else than JSON.
//!
//! Nothing here is written back: the view is read-only, and `routinectl`
//! stays the way to change anything.

use parking_lot::RwLock;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// How stale a snapshot may get before a look at the view refreshes it.
/// Routines change by hand, minutes apart at best; the refresh shells out to
/// `routinectl`, so it stays off the tick.
const MAX_AGE_SECS: u64 = 30;

const MONTHS: [&str; 12] =
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// A local wall-clock instant, minute precision: enough for every word here
/// ("in 20 min", "Wed 09:03") and no time-zone arithmetic to get wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct When {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

impl When {
    /// The local clock, to the minute.
    pub fn now() -> Self {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as libc::time_t)
            .unwrap_or(0);
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&secs, &mut tm) };
        When {
            year: tm.tm_year + 1900,
            month: (tm.tm_mon + 1) as u32,
            day: tm.tm_mday as u32,
            hour: tm.tm_hour as u32,
            minute: tm.tm_min as u32,
        }
    }

    /// Parse routinectl's local ISO shape: `2026-11-04T07:43`, with or
    /// without seconds. Anything else is `None` rather than a guess.
    pub fn parse(s: &str) -> Option<Self> {
        let (date, time) = s.split_once('T')?;
        let mut d = date.split('-');
        let year: i32 = d.next()?.parse().ok()?;
        let month: u32 = d.next()?.parse().ok()?;
        let day: u32 = d.next()?.parse().ok()?;
        let mut t = time.split(':');
        let hour: u32 = t.next()?.parse().ok()?;
        let minute: u32 = t.next()?.parse().ok()?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
            return None;
        }
        Some(When { year, month, day, hour, minute })
    }

    /// `2026-09-26 10:04`, the date and time a row shows.
    pub fn words(&self) -> String {
        format!("{:04}-{:02}-{:02} {:02}:{:02}", self.year, self.month, self.day, self.hour, self.minute)
    }

    /// Days since 1970-01-01, the calendar day arithmetic these words need.
    fn day_number(&self) -> i64 {
        let y = if self.month <= 2 { self.year as i64 - 1 } else { self.year as i64 };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = (self.month as i64 + 9) % 12;
        let doy = (153 * mp + 2) / 5 + self.day as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }

    /// The epoch second this local minute starts at. `mktime` reads the fields
    /// in the local zone, which is the zone routinectl printed them in, so a
    /// routine's last run compares directly with a pane's or an ask's clock.
    /// Out of range (a year before 1970, a broken timestamp) is 0: the needs
    /// model then simply sorts it first.
    pub fn epoch_secs(&self) -> u64 {
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        tm.tm_year = self.year - 1900;
        tm.tm_mon = self.month as i32 - 1;
        tm.tm_mday = self.day as i32;
        tm.tm_hour = self.hour as i32;
        tm.tm_min = self.minute as i32;
        tm.tm_isdst = -1;
        let secs = unsafe { libc::mktime(&mut tm) };
        if secs < 0 { 0 } else { secs as u64 }
    }

    fn minute_number(&self) -> i64 {
        self.day_number() * 1440 + self.hour as i64 * 60 + self.minute as i64
    }

    fn weekday(&self) -> &'static str {
        // 1970-01-01 was a Thursday.
        WEEKDAYS[(self.day_number() + 4).rem_euclid(7) as usize]
    }
}

/// The last run routinectl reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LastRun {
    pub start: Option<When>,
    pub end: Option<When>,
    pub code: i32,
}

impl LastRun {
    /// When it finished, or when it started if it has not: a run still going
    /// has no end, and a killed one may never get one.
    pub fn at(&self) -> Option<When> {
        self.end.or(self.start)
    }
}

/// How the last run ended, in the four states a row can show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LastStatus {
    #[default]
    Never,
    Ok,
    Failed(i32),
    Running,
}

/// Where a routine belongs in the Scheduled list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    /// The last run failed and nothing paused it: the one group that asks
    /// for a look.
    NeedsAttention,
    /// Next run later today.
    Today,
    /// Next run in the coming seven days.
    ThisWeek,
    /// Further out, or a manual routine with no next run at all.
    #[default]
    Later,
    /// Switched off on purpose (`enabled: false`).
    Off,
}

/// One scheduled routine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Routine {
    /// The name passed to the runner, which also names the prompt and the log.
    pub name: String,
    /// launchd's label, the handle `launchctl` takes. Empty from routinectl,
    /// which does not report it.
    pub label: String,
    /// When it runs, in words ("daily 09:07").
    pub schedule: String,
    /// The same, phrased by routinectl for a human ("once, 6 Nov 09:47").
    pub schedule_words: String,
    /// True when launchd has it loaded. A plist alone never fires.
    pub loaded: bool,
    /// True when `<name>.done` neutralises it, or when its plist is parked
    /// as `.plist.disabled`: switched off on purpose, not broken.
    pub done: bool,
    /// True when the prompt file is missing: the run would fail at once.
    pub prompt_missing: bool,
    /// True while a run is going.
    pub running: bool,
    /// True when `enabled` is false: off, and it says so.
    pub paused: bool,
    /// True for a routine with no plist, run by hand (`routinectl run`).
    pub manual: bool,
    /// The next run, local time; `None` for a manual routine.
    pub next_run: Option<When>,
    /// The last run, when routinectl or the log knows of one.
    pub last: Option<LastRun>,
    /// Where it runs, which decides, for a Claude routine, whose accounts it
    /// speaks with.
    pub cwd: String,
    /// The prompt it is handed, verbatim; empty when the file is gone.
    pub prompt: String,
    /// The log file routinectl reports.
    pub log: String,
    /// Extra MCP tools granted to it, one per line of `prompts/<name>.tools`.
    pub tools: Vec<String>,
    /// Extra writable directories, from `prompts/<name>.dirs`.
    pub dirs: Vec<String>,
}

impl Routine {
    /// What is wrong with this routine, if anything: the line the view shows
    /// in place of the schedule. A routine switched off on purpose (`.done`,
    /// paused, or manual) has no problem: its state already says so.
    pub fn problem(&self) -> Option<&'static str> {
        if self.done || self.paused || self.manual {
            None
        } else if self.prompt_missing {
            Some("no prompt")
        } else if !self.loaded {
            Some("not loaded")
        } else {
            None
        }
    }
}

/// Where a routine sits for the sidebar, given the clock. A failure wins over
/// every schedule group, a paused routine is off whatever its last run did,
/// and a manual routine has no next run to be today or later.
pub fn group(r: &Routine, now: When) -> Group {
    if !r.paused && r.last.as_ref().is_some_and(|l| l.code != 0) {
        return Group::NeedsAttention;
    }
    if r.paused {
        return Group::Off;
    }
    if r.manual {
        return Group::Later;
    }
    match r.next_run {
        Some(next) => {
            let days = next.day_number() - now.day_number();
            if days == 0 {
                Group::Today
            } else if (1..=7).contains(&days) {
                Group::ThisWeek
            } else {
                Group::Later
            }
        }
        None => Group::Later,
    }
}

/// When it next runs: `19:05` today, `Wed 09:03` within seven days,
/// `9 Nov` further out.
pub fn next_words(next: When, now: When) -> String {
    let days = next.day_number() - now.day_number();
    if days == 0 {
        format!("{:02}:{:02}", next.hour, next.minute)
    } else if (1..=7).contains(&days) {
        format!("{} {:02}:{:02}", next.weekday(), next.hour, next.minute)
    } else {
        format!("{} {}", next.day, MONTHS[(next.month - 1) as usize])
    }
}

/// How long until it runs: `in 20 min`, `in 2 h`, `in 3 d`, `in 2 mo`.
pub fn in_words(next: When, now: When) -> String {
    let mins = (next.minute_number() - now.minute_number()).max(0);
    if mins < 1 {
        "now".into()
    } else if mins < 60 {
        format!("in {mins} min")
    } else if mins < 1440 {
        format!("in {} h", mins / 60)
    } else if mins < 30 * 1440 {
        format!("in {} d", mins / 1440)
    } else {
        format!("in {} mo", (mins / 1440 / 30).max(1))
    }
}

/// How long ago something happened: `just now`, `10 min ago`, `9 h ago`,
/// `2 d ago`.
pub fn ago_words(then: When, now: When) -> String {
    let mins = (now.minute_number() - then.minute_number()).max(0);
    if mins == 0 {
        "just now".to_string()
    } else if mins < 60 {
        format!("{mins} min ago")
    } else if mins < 1440 {
        format!("{} h ago", mins / 60)
    } else if mins < 30 * 1440 {
        format!("{} d ago", mins / 1440)
    } else {
        format!("{} mo ago", (mins / 1440 / 30).max(1))
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}

fn routines_dir() -> PathBuf {
    home().join(".claude/routines")
}

static CACHE: RwLock<Option<Arc<Vec<Routine>>>> = RwLock::new(None);
/// When the cache was last filled, epoch seconds. 0 means never.
static FILLED_AT: AtomicU64 = AtomicU64::new(0);
/// Set while a refresh thread is running, so a tick never starts a second one.
static REFRESHING: AtomicBool = AtomicBool::new(false);
/// Set once the fallback to the disk reader has been logged, so a routinectl
/// that stays missing does not fill the log with the same line every 30 s.
static FALLBACK_LOGGED: AtomicBool = AtomicBool::new(false);

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The routines as last read, and a refresh in the background when the
/// snapshot has aged out. Returns at once, and `None` until the first read
/// lands: reading them shells out, which has no business on the tick, and
/// "not read yet" must not be shown as "nothing scheduled".
pub fn snapshot() -> Option<Arc<Vec<Routine>>> {
    let age = now_secs().saturating_sub(FILLED_AT.load(Ordering::Relaxed));
    if age >= MAX_AGE_SECS && !REFRESHING.swap(true, Ordering::AcqRel) {
        std::thread::spawn(|| {
            let list = read_all();
            *CACHE.write() = Some(Arc::new(list));
            FILLED_AT.store(now_secs(), Ordering::Relaxed);
            REFRESHING.store(false, Ordering::Release);
        });
    }
    CACHE.read().clone()
}

/// Read every routine, from routinectl when it can, from disk otherwise.
/// Blocking: background only.
fn read_all() -> Vec<Routine> {
    match read_from_routinectl() {
        Ok(list) => list,
        Err(why) => {
            if !FALLBACK_LOGGED.swap(true, Ordering::AcqRel) {
                log::warn!("routines: {why}; falling back to plists and launchd");
            }
            read_from_disk()
        }
    }
}

/// `routinectl list --json`, argv only: the path comes from `$HOME` and no
/// shell is involved.
fn read_from_routinectl() -> Result<Vec<Routine>, String> {
    let bin = routines_dir().join("routinectl");
    let out = std::process::Command::new(&bin)
        .args(["list", "--json"])
        .output()
        .map_err(|e| format!("cannot run {}: {e}", bin.display()))?;
    if !out.status.success() {
        return Err(format!("{} list --json exited with {}", bin.display(), out.status));
    }
    let text = String::from_utf8(out.stdout).map_err(|_| "routinectl printed non-UTF-8".to_string())?;
    parse_list(&text).ok_or_else(|| "routinectl printed invalid JSON".to_string())
}

/// The fields `routinectl list --json` prints, before dates are checked.
#[derive(Debug, serde::Deserialize)]
struct WireRoutine {
    name: String,
    #[serde(default)]
    schedule: String,
    #[serde(default)]
    schedule_words: String,
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    manual: bool,
    #[serde(default = "yes")]
    loaded: bool,
    #[serde(default)]
    running: bool,
    #[serde(default)]
    next_run: Option<String>,
    #[serde(default)]
    last: Option<WireLast>,
    #[serde(default)]
    cwd: String,
    /// The prompt file's path; its text is what `Routine::prompt` carries.
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    log: String,
}

#[derive(Debug, serde::Deserialize)]
struct WireLast {
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
    #[serde(default)]
    code: i32,
}

fn yes() -> bool {
    true
}

/// Parse routinectl's JSON. `None` on anything malformed, so the caller can
/// fall back instead of showing half a list. Done routines are dropped.
fn parse_list(json: &str) -> Option<Vec<Routine>> {
    let wire: Vec<WireRoutine> = serde_json::from_str(json).ok()?;
    let mut out = Vec::with_capacity(wire.len());
    for w in wire {
        if let Some(r) = routine_from_wire(w).ok()? {
            out.push(r);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Some(out)
}

fn routine_from_wire(w: WireRoutine) -> Result<Option<Routine>, ()> {
    // A finished routine is off the list for good: it belongs to no group.
    if w.done {
        return Ok(None);
    }
    let next_run = match w.next_run.as_deref() {
        Some(s) => Some(When::parse(s).ok_or(())?),
        None => None,
    };
    let last = match &w.last {
        Some(l) => Some(LastRun {
            start: match l.start.as_deref() {
                Some(s) => Some(When::parse(s).ok_or(())?),
                None => None,
            },
            end: match l.end.as_deref() {
                Some(s) => Some(When::parse(s).ok_or(())?),
                None => None,
            },
            code: l.code,
        }),
        None => None,
    };
    let prompt_path = PathBuf::from(&w.prompt);
    let prompt_missing = w.prompt.is_empty() || !prompt_path.is_file();
    let prompt = if prompt_missing {
        String::new()
    } else {
        std::fs::read_to_string(&prompt_path).unwrap_or_default()
    };
    Ok(Some(Routine {
        name: w.name,
        label: String::new(),
        schedule: w.schedule,
        schedule_words: w.schedule_words,
        loaded: w.loaded,
        done: false,
        prompt_missing,
        running: w.running,
        paused: !w.enabled,
        manual: w.manual,
        next_run,
        last,
        cwd: w.cwd,
        prompt,
        log: w.log,
        // The sidecars sit next to the prompt: `prompts/<name>.tools` and
        // `prompts/<name>.dirs`.
        tools: lines_of(&prompt_path.with_extension("tools")),
        dirs: lines_of(&prompt_path.with_extension("dirs")),
    }))
}

/// Read every routine from disk and launchd, the reader used when routinectl
/// is missing or prints something else than JSON. Blocking: background only.
fn read_from_disk() -> Vec<Routine> {
    let loaded = loaded_labels();
    let dir = routines_dir();
    let mut out: Vec<Routine> = Vec::new();

    let agents = home().join("Library/LaunchAgents");
    let Ok(entries) = std::fs::read_dir(&agents) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        // A plist parked as `<label>.plist.disabled` is off on purpose. It
        // still belongs on the list (a routine you switched off and forgot
        // is exactly what this view is for), so read it and mark it.
        let ext = path.extension().and_then(|e| e.to_str());
        let disabled = ext == Some("disabled")
            && path.file_stem().is_some_and(|s| Path::new(s).extension().and_then(|e| e.to_str()) == Some("plist"));
        if ext != Some("plist") && !disabled {
            continue;
        }
        let Some(json) = plist_json(&path) else { continue };
        let Some((label, name, schedule)) = parse_agent(&json) else { continue };
        let log = dir.join(format!("logs/{name}.log"));
        let prompt_path = dir.join(format!("prompts/{name}.md"));
        let prompt_missing = !prompt_path.exists();
        out.push(Routine {
            loaded: !disabled && loaded.iter().any(|l| l == &label),
            done: disabled || dir.join(format!("{name}.done")).exists(),
            prompt_missing,
            running: false,
            paused: false,
            manual: false,
            next_run: None,
            last: std::fs::read_to_string(&log).ok().as_deref().and_then(last_from_log),
            prompt: std::fs::read_to_string(&prompt_path).unwrap_or_default(),
            tools: lines_of(&dir.join(format!("prompts/{name}.tools"))),
            dirs: lines_of(&dir.join(format!("prompts/{name}.dirs"))),
            cwd: json
                .get("ProgramArguments")
                .and_then(|a| a.as_array())
                .and_then(|a| a.get(3))
                .and_then(|w| w.as_str())
                .unwrap_or_default()
                .to_string(),
            log: log.display().to_string(),
            name,
            label,
            schedule,
            schedule_words: String::new(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The meaningful lines of a sidecar file: the runner skips blanks and `#`
/// comments, so the view must not show what the run will ignore.
fn lines_of(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect()
}

/// `launchctl list`, reduced to the labels it knows.
fn loaded_labels() -> Vec<String> {
    let Ok(out) = std::process::Command::new("/bin/launchctl").arg("list").output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .skip(1)
        .filter_map(|l| l.split('\t').nth(2))
        .map(|s| s.trim().to_string())
        .collect()
}

/// A plist as JSON, via `plutil`: no plist parser to carry for six files.
fn plist_json(path: &Path) -> Option<serde_json::Value> {
    let out = std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

/// (label, routine name, schedule) for a plist that runs the routine runner,
/// `None` for every other launch agent.
fn parse_agent(v: &serde_json::Value) -> Option<(String, String, String)> {
    let args: Vec<&str> = v.get("ProgramArguments")?.as_array()?.iter().filter_map(|a| a.as_str()).collect();
    let script = args.iter().position(|a| a.ends_with("run-routine.sh"))?;
    let name = (*args.get(script + 1)?).to_string();
    let label = v.get("Label").and_then(|l| l.as_str()).unwrap_or(&name).to_string();
    Some((label, name, schedule_of(v)))
}

/// The schedule in words. `StartInterval` is plain seconds;
/// `StartCalendarInterval` is one dict or an array of them, and an absent key
/// there means "every one of those", so a bare `Hour`/`Minute` is daily.
pub fn schedule_of(v: &serde_json::Value) -> String {
    if let Some(secs) = v.get("StartInterval").and_then(|s| s.as_i64()) {
        return if secs % 3600 == 0 {
            format!("every {} h", secs / 3600)
        } else {
            format!("every {} min", (secs / 60).max(1))
        };
    }
    let Some(cal) = v.get("StartCalendarInterval") else { return "at load".into() };
    let entries: Vec<&serde_json::Value> = match cal.as_array() {
        Some(a) => a.iter().collect(),
        None => vec![cal],
    };
    let parts: Vec<String> = entries.iter().map(|e| one_calendar(e)).collect();
    if parts.is_empty() {
        return "at load".into();
    }
    // Several times the same day: say the day once ("every day 10:02 and
    // 15:02"), not the whole phrase twice.
    let day_of = |s: &String| s.rsplit_once(' ').map(|(d, _)| d.to_string()).unwrap_or_default();
    let first_day = day_of(&parts[0]);
    if parts.len() > 1 && parts.iter().all(|p| day_of(p) == first_day) {
        let times: Vec<&str> = parts.iter().map(|p| p.rsplit(' ').next().unwrap_or("")).collect();
        return format!("{first_day} {}", times.join(" and "));
    }
    parts.join(", ")
}

fn one_calendar(e: &serde_json::Value) -> String {
    let num = |k: &str| e.get(k).and_then(|n| n.as_i64());
    let (h, m) = (num("Hour").unwrap_or(0), num("Minute").unwrap_or(0));
    let mut when = String::new();
    if let Some(d) = num("Day") {
        when.push_str(&format!("{d} "));
        let name = num("Month").and_then(|mo| MONTHS.get((mo - 1).clamp(0, 11) as usize));
        if let Some(name) = name {
            when.push_str(name);
            when.push(' ');
        }
    } else if let Some(wd) = num("Weekday") {
        const DAYS: [&str; 7] = ["Sundays", "Mondays", "Tuesdays", "Wednesdays", "Thursdays", "Fridays", "Saturdays"];
        when.push_str(DAYS.get((wd % 7) as usize).copied().unwrap_or("every day"));
        when.push(' ');
    } else {
        when.push_str("every day ");
    }
    // 24-hour, zero-padded: the rows line up under each other.
    format!("{when}{h:02}:{m:02}")
}

/// The end of the last run in a log: the runner writes
/// `=== <date> <time> fin de <name> (code N)` when the agent returns.
pub fn last_run(log: &str) -> Option<(String, i32)> {
    let line = log.lines().rev().find(|l| l.starts_with("=== ") && l.contains(" fin de "))?;
    let mut fields = line.split_whitespace().skip(1);
    let date = fields.next()?;
    let time = fields.next()?;
    let code = line
        .rsplit_once("(code ")
        .and_then(|(_, rest)| rest.trim_end_matches(')').trim().parse().ok())
        .unwrap_or(0);
    Some((format!("{date} {time}"), code))
}

/// The same end line as a `LastRun`: the log only knows the end and the
/// code, the start is left out.
fn last_from_log(log: &str) -> Option<LastRun> {
    let (words, code) = last_run(log)?;
    let end = When::parse(&words.replacen(' ', "T", 1))?;
    Some(LastRun { start: None, end: Some(end), code })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(s: &str) -> When {
        When::parse(s).expect("a date")
    }

    fn routine(name: &str) -> Routine {
        Routine { name: name.into(), loaded: true, ..Default::default() }
    }

    #[test]
    fn routinectl_dates_are_read_to_the_minute() {
        assert_eq!(
            When::parse("2026-11-04T07:43"),
            Some(When { year: 2026, month: 11, day: 4, hour: 7, minute: 43 })
        );
        // The last run carries seconds; they are dropped, not rejected.
        assert_eq!(at("2026-11-03T07:43:27").minute, 43);
        assert_eq!(When::parse("2026-13-01T00:00"), None);
        assert_eq!(When::parse("tomorrow"), None);
    }

    #[test]
    fn a_local_minute_reads_back_from_its_epoch_second() {
        // Zoned through libc both ways, so the test holds whatever the Mac's
        // zone is: what matters is that the two agree, not which instant it is.
        let when = When { year: 2026, month: 11, day: 3, hour: 9, minute: 47 };
        let secs = when.epoch_secs();
        assert!(secs > 0);
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let t = secs as libc::time_t;
        unsafe { libc::localtime_r(&t, &mut tm) };
        assert_eq!(
            (
                tm.tm_year + 1900,
                (tm.tm_mon + 1) as u32,
                tm.tm_mday as u32,
                tm.tm_hour as u32,
                tm.tm_min as u32
            ),
            (2026, 11, 3, 9, 47)
        );
    }

    #[test]
    fn groups_follow_the_next_run_and_the_last_failure() {
        let now = at("2026-11-03T20:30");
        let mut r = routine("alpha");
        r.next_run = Some(at("2026-11-03T21:05"));
        assert_eq!(group(&r, now), Group::Today);
        r.next_run = Some(at("2026-11-06T09:47"));
        assert_eq!(group(&r, now), Group::ThisWeek);
        // Seven days out still counts as this week; eight does not.
        r.next_run = Some(at("2026-11-10T09:03"));
        assert_eq!(group(&r, now), Group::ThisWeek);
        r.next_run = Some(at("2026-11-11T09:03"));
        assert_eq!(group(&r, now), Group::Later);
        // A manual routine has no next run to be today.
        r.manual = true;
        r.next_run = Some(at("2026-11-03T21:05"));
        assert_eq!(group(&r, now), Group::Later);
        r.manual = false;
        // A failed run asks for a look even when the next one is today.
        r.last = Some(LastRun { start: None, end: Some(at("2026-11-03T19:00")), code: 2 });
        assert_eq!(group(&r, now), Group::NeedsAttention);
        // Paused wins over a failure: off is not broken.
        r.paused = true;
        assert_eq!(group(&r, now), Group::Off);
    }

    #[test]
    fn next_words_name_today_the_week_then_the_date() {
        let now = at("2026-11-03T20:30");
        assert_eq!(next_words(at("2026-11-03T21:05"), now), "21:05");
        assert_eq!(next_words(at("2026-11-05T09:03"), now), "Thu 09:03");
        assert_eq!(next_words(at("2026-11-10T09:03"), now), "Tue 09:03");
        assert_eq!(next_words(at("2026-11-11T09:03"), now), "11 Nov");
    }

    #[test]
    fn in_words_pick_the_unit_that_fits() {
        let now = at("2026-11-03T20:30");
        assert_eq!(in_words(at("2026-11-03T20:50"), now), "in 20 min");
        assert_eq!(in_words(at("2026-11-03T22:30"), now), "in 2 h");
        assert_eq!(in_words(at("2026-11-06T20:30"), now), "in 3 d");
        assert_eq!(in_words(at("2027-01-02T20:30"), now), "in 2 mo");
        assert_eq!(in_words(now, now), "now");
    }

    #[test]
    fn ago_words_pick_the_unit_that_fits() {
        let now = at("2026-11-03T20:30");
        assert_eq!(ago_words(now, now), "just now");
        assert_eq!(ago_words(at("2026-11-03T20:20"), now), "10 min ago");
        assert_eq!(ago_words(at("2026-11-03T11:30"), now), "9 h ago");
        assert_eq!(ago_words(at("2026-11-01T20:30"), now), "2 d ago");
    }

    #[test]
    fn the_fixture_reads_every_shape_and_drops_the_done_one() {
        let list = parse_list(include_str!("../tests/fixtures/routines/list.json")).expect("valid JSON");
        let names: Vec<&str> = list.iter().map(|r| r.name.as_str()).collect();
        // Sorted by name, the done routine gone.
        assert_eq!(
            names,
            ["alpha-digest", "beta-cleanup", "delta-paused", "epsilon-manual", "eta-week", "gamma-report", "theta-edge", "zeta-later"]
        );
        let find = |name: &str| list.iter().find(|r| r.name == name).expect("a fixture routine");

        let alpha = find("alpha-digest");
        assert!(!alpha.paused);
        assert!(!alpha.manual);
        assert!(!alpha.running);
        assert_eq!(alpha.schedule_words, "2 times a day");
        assert_eq!(alpha.cwd, "/run/alpha");
        assert_eq!(alpha.log, "/logs/alpha-digest.log");
        assert_eq!(alpha.last, Some(LastRun {
            start: Some(at("2026-11-03T09:07:04")),
            end: Some(at("2026-11-03T09:07:31")),
            code: 0,
        }));

        let beta = find("beta-cleanup");
        assert!(beta.running);
        assert_eq!(beta.last.and_then(|l| l.at()), Some(at("2026-11-03T20:20:00")));

        let gamma = find("gamma-report");
        assert_eq!(gamma.last.map(|l| l.code), Some(2));

        assert!(find("delta-paused").paused);
        let manual = find("epsilon-manual");
        assert!(manual.manual);
        assert_eq!(manual.next_run, None);
        assert_eq!(find("zeta-later").last, None);
    }

    #[test]
    fn the_fixture_groups_at_a_fixed_clock() {
        let now = at("2026-11-03T20:30");
        let list = parse_list(include_str!("../tests/fixtures/routines/list.json")).expect("valid JSON");
        let groups: Vec<(&str, Group)> = list.iter().map(|r| (r.name.as_str(), group(r, now))).collect();
        assert_eq!(
            groups,
            [
                ("alpha-digest", Group::Today),
                ("beta-cleanup", Group::Today),
                ("delta-paused", Group::Off),
                ("epsilon-manual", Group::Later),
                ("eta-week", Group::ThisWeek),
                ("gamma-report", Group::NeedsAttention),
                ("theta-edge", Group::ThisWeek),
                ("zeta-later", Group::Later),
            ]
        );
    }

    #[test]
    fn a_bare_hour_and_minute_means_every_day() {
        assert_eq!(schedule_of(&json!({"StartCalendarInterval": {"Hour": 9, "Minute": 7}})), "every day 09:07");
    }

    #[test]
    fn a_weekday_and_a_date_name_themselves() {
        assert_eq!(schedule_of(&json!({"StartCalendarInterval": {"Weekday": 1, "Hour": 8, "Minute": 3}})), "Mondays 08:03");
        assert_eq!(
            schedule_of(&json!({"StartCalendarInterval": {"Month": 10, "Day": 1, "Hour": 9, "Minute": 3}})),
            "1 Oct 09:03"
        );
    }

    #[test]
    fn several_times_the_same_day_say_the_day_once() {
        let v = json!({"StartCalendarInterval": [
            {"Hour": 10, "Minute": 2},
            {"Hour": 15, "Minute": 2},
        ]});
        assert_eq!(schedule_of(&v), "every day 10:02 and 15:02");
    }

    #[test]
    fn different_days_keep_their_own_phrase() {
        let v = json!({"StartCalendarInterval": [
            {"Weekday": 1, "Hour": 8, "Minute": 3},
            {"Weekday": 5, "Hour": 17, "Minute": 3},
        ]});
        assert_eq!(schedule_of(&v), "Mondays 08:03, Fridays 17:03");
    }

    #[test]
    fn an_interval_is_read_in_hours_when_it_divides() {
        assert_eq!(schedule_of(&json!({"StartInterval": 7200})), "every 2 h");
        assert_eq!(schedule_of(&json!({"StartInterval": 900})), "every 15 min");
    }

    #[test]
    fn only_an_agent_running_the_routine_runner_counts() {
        let ours = json!({
            "Label": "com.example.routine.alpha-digest",
            "ProgramArguments": ["/bin/bash", "/Users/x/.claude/routines/run-routine.sh", "alpha-digest", "/Users/x/Perso"],
            "StartCalendarInterval": {"Hour": 10, "Minute": 2},
        });
        let parsed = parse_agent(&ours).expect("a routine");
        assert_eq!(parsed.0, "com.example.routine.alpha-digest");
        assert_eq!(parsed.1, "alpha-digest");
        assert_eq!(parsed.2, "every day 10:02");

        // Somebody else's launch agent is not a routine.
        let other = json!({"Label": "com.apple.thing", "ProgramArguments": ["/usr/bin/true"]});
        assert!(parse_agent(&other).is_none());
    }

    #[test]
    fn a_sidecar_file_yields_only_the_lines_the_runner_would_use() {
        let dir = std::env::temp_dir().join(format!("kova-routines-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.tools");
        std::fs::write(&f, "# les outils\nmcp__gmail__search\n\n  mcp__drive__read  \n#mcp__off\n").unwrap();
        assert_eq!(lines_of(&f), vec!["mcp__gmail__search", "mcp__drive__read"]);
        // A routine with no sidecar simply has none.
        assert!(lines_of(&dir.join("absent.tools")).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_last_run_is_the_last_end_line_of_the_log() {
        let log = "=== 2026-09-25 10:02:01 démarrage de alpha-digest\n\
                   === 2026-09-25 10:04:34 fin de alpha-digest (code 0)\n\
                   === 2026-09-26 10:02:01 démarrage de alpha-digest\n\
                   === 2026-09-26 10:05:12 fin de alpha-digest (code 2)\n";
        assert_eq!(last_run(log), Some(("2026-09-26 10:05:12".into(), 2)));
        // A run still going has no end line yet.
        assert_eq!(last_run("=== 2026-09-26 10:02:01 démarrage de alpha-digest\n"), None);
        assert_eq!(
            last_from_log(log),
            Some(LastRun { start: None, end: Some(at("2026-09-26T10:05:12")), code: 2 })
        );
    }

    #[test]
    fn a_routine_reports_what_keeps_it_from_running() {
        let mut r = Routine { loaded: true, ..Default::default() };
        assert_eq!(r.problem(), None);
        r.loaded = false;
        assert_eq!(r.problem(), Some("not loaded"));
        r.prompt_missing = true;
        assert_eq!(r.problem(), Some("no prompt"));
        // Switched off on purpose is not a fault to report.
        r.done = true;
        assert_eq!(r.problem(), None);
        r.done = false;
        r.paused = true;
        assert_eq!(r.problem(), None);
        r.paused = false;
        r.manual = true;
        assert_eq!(r.problem(), None);
    }

}
