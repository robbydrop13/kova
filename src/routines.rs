//! The scheduled local routines, read for the sidebar's Routines view.
//!
//! A routine is a launchd agent that runs `~/.claude/routines/run-routine.sh
//! <name> <workdir>` on a calendar schedule. Its definition is spread over
//! three places that drift apart in silence: the plist in `~/Library/
//! LaunchAgents`, the prompt in `~/.claude/routines/prompts`, and launchd
//! itself, which knows nothing of a plist until it is loaded. This module
//! reads all three so the sidebar can show a routine that will never fire.
//!
//! Agents are found by what they run, not by a label prefix: any plist whose
//! `ProgramArguments` names `run-routine.sh` is one. Nothing here is written
//! back — the view is read-only, and `launchctl` stays the way to change
//! anything.

use parking_lot::RwLock;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// How stale a snapshot may get before a look at the view refreshes it.
/// Routines change by hand, minutes apart at best; the refresh shells out to
/// `launchctl` and `plutil`, so it stays off the tick.
const MAX_AGE_SECS: u64 = 30;

/// One scheduled routine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Routine {
    /// The name passed to the runner, which also names the prompt and the log.
    pub name: String,
    /// launchd's label, the handle `launchctl` takes.
    pub label: String,
    /// When it runs, in words ("every day 09:07").
    pub schedule: String,
    /// True when launchd has it loaded. A plist alone never fires.
    pub loaded: bool,
    /// True when `<name>.done` neutralises it, or when its plist is parked
    /// as `.plist.disabled`: switched off on purpose, not broken.
    pub done: bool,
    /// True when the prompt file is missing: the run would fail at once.
    pub prompt_missing: bool,
    /// The last run: the time it ended and its exit code, from the log.
    pub last_run: Option<(String, i32)>,
}

impl Routine {
    /// What is wrong with this routine, if anything — the line the view shows
    /// in place of the schedule. A routine switched off on purpose has no
    /// problem: `done` already says so, and its schedule stays readable.
    pub fn problem(&self) -> Option<&'static str> {
        if self.done {
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
static REFRESHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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

/// Read every routine from disk and launchd. Blocking: background only.
fn read_all() -> Vec<Routine> {
    let loaded = loaded_labels();
    let dir = routines_dir();
    let mut out: Vec<Routine> = Vec::new();

    let agents = home().join("Library/LaunchAgents");
    let Ok(entries) = std::fs::read_dir(&agents) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        // A plist parked as `<label>.plist.disabled` is off on purpose. It
        // still belongs on the list — a routine you switched off and forgot
        // is exactly what this view is for — so read it and mark it.
        let ext = path.extension().and_then(|e| e.to_str());
        let disabled = ext == Some("disabled") && path.file_stem().is_some_and(|s| Path::new(s).extension().and_then(|e| e.to_str()) == Some("plist"));
        if ext != Some("plist") && !disabled {
            continue;
        }
        let Some(json) = plist_json(&path) else { continue };
        let Some((label, name, schedule)) = parse_agent(&json) else { continue };
        let log = dir.join(format!("logs/{name}.log"));
        out.push(Routine {
            loaded: !disabled && loaded.iter().any(|l| l == &label),
            done: disabled || dir.join(format!("{name}.done")).exists(),
            prompt_missing: !dir.join(format!("prompts/{name}.md")).exists(),
            last_run: std::fs::read_to_string(&log).ok().as_deref().and_then(last_run),
            name,
            label,
            schedule,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
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

/// A plist as JSON, via `plutil` — no plist parser to carry for six files.
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
        if let Some(mo) = num("Month") {
            const MONTHS: [&str; 12] =
                ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
            if let Some(name) = MONTHS.get((mo - 1).clamp(0, 11) as usize) {
                when.push_str(name);
                when.push(' ');
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
            "Label": "com.robin.routine.tri-inbox",
            "ProgramArguments": ["/bin/bash", "/Users/x/.claude/routines/run-routine.sh", "tri-inbox", "/Users/x/Perso"],
            "StartCalendarInterval": {"Hour": 10, "Minute": 2},
        });
        let parsed = parse_agent(&ours).expect("a routine");
        assert_eq!(parsed.0, "com.robin.routine.tri-inbox");
        assert_eq!(parsed.1, "tri-inbox");
        assert_eq!(parsed.2, "every day 10:02");

        // Somebody else's launch agent is not a routine.
        let other = json!({"Label": "com.apple.thing", "ProgramArguments": ["/usr/bin/true"]});
        assert!(parse_agent(&other).is_none());
    }

    #[test]
    fn the_last_run_is_the_last_end_line_of_the_log() {
        let log = "=== 2026-09-25 10:02:01 démarrage de tri-inbox\n\
                   === 2026-09-25 10:04:34 fin de tri-inbox (code 0)\n\
                   === 2026-09-26 10:02:01 démarrage de tri-inbox\n\
                   === 2026-09-26 10:05:12 fin de tri-inbox (code 2)\n";
        assert_eq!(last_run(log), Some(("2026-09-26 10:05:12".into(), 2)));
        // A run still going has no end line yet.
        assert_eq!(last_run("=== 2026-09-26 10:02:01 démarrage de tri-inbox\n"), None);
    }

    #[test]
    fn a_routine_reports_what_keeps_it_from_running() {
        let mut r = Routine { loaded: true, ..Default::default() };
        assert_eq!(r.problem(), None);
        r.loaded = false;
        assert_eq!(r.problem(), Some("not loaded"));
        r.prompt_missing = true;
        assert_eq!(r.problem(), Some("no prompt"));
        // Switched off on purpose is not a fault to report: `done` says it,
        // and the row keeps showing when it would have run.
        r.done = true;
        assert_eq!(r.problem(), None);
    }
}
