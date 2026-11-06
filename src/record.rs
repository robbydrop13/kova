//! The Cockpit's `Enregistrer` button (`docs/cockpit.md` § Record): Kova
//! records the screen, the default microphone and the clicks with macOS's own
//! `screencapture -v`, for the montre-une-fois skill. Stop sends SIGINT, the
//! file is waited for until its size settles, then it is handed to the chief:
//! `attn add` (a memo with the path) and `attn act <id> instruct`, which types
//! the instruction into claap-agent's session.
//!
//! Nothing records without the click: the state lives in memory and starts
//! `Idle` at every launch. Every command is an argv, never a shell line, so a
//! path with spaces is one element. The phases are a pure state machine
//! (`Phase::next`), tested; the process handling around it is thin.

use std::path::{Path, PathBuf};
use std::process::Child;

use parking_lot::Mutex;

/// macOS's built-in recorder.
pub const SCREENCAPTURE: &str = "/usr/sbin/screencapture";
/// What the user reads when the capture produced nothing: almost always a
/// permission macOS has not granted yet. Agents never change these settings.
pub const PERMISSION_LINE: &str =
    "Autorise Kova dans Réglages Système > Confidentialité : Enregistrement de l'écran et Microphone, puis réessaie";
/// The chief the recording goes to.
pub const CHIEF: &str = "claap-agent";
const ENTRY_TITLE: &str = "Enregistrement à transformer en skill";

/// Where the recording stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Recording { path: PathBuf, started_ms: u64 },
    /// Stopped: waiting for screencapture to exit and the file to settle.
    Finalizing { path: PathBuf },
    /// The file is good: `attn add` and `attn act` run.
    Delivering { path: PathBuf },
    /// `note`: what `attn act` said (`remis`, or when it will be).
    Delivered { path: PathBuf, note: String },
    Failed { failure: Failure, path: Option<PathBuf> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// screencapture did not start, or left no file: `PERMISSION_LINE`.
    Capture,
    /// The file is there but did not reach the chief: the CLI's words.
    Delivery(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The click on `Enregistrer`, once screencapture runs.
    Started { path: PathBuf, at_ms: u64 },
    /// screencapture could not be spawned.
    StartFailed,
    /// The click on `Arrêter`, or screencapture exited on its own.
    Stopped,
    /// The file settled (`true`) or is missing or empty (`false`).
    Captured(bool),
    /// The hand-off ended: `attn act`'s words, or why it failed.
    Delivered(Result<String, String>),
    /// The note is closed.
    Dismiss,
}

impl Phase {
    /// The phase after `event`; an event that does not apply leaves it as it
    /// is (a second Stop, a Start while recording).
    pub fn next(self, event: Event) -> Phase {
        match (self, event) {
            (Phase::Idle | Phase::Delivered { .. } | Phase::Failed { .. }, Event::Started { path, at_ms }) => {
                Phase::Recording { path, started_ms: at_ms }
            }
            (Phase::Idle | Phase::Delivered { .. } | Phase::Failed { .. }, Event::StartFailed) => {
                Phase::Failed { failure: Failure::Capture, path: None }
            }
            (Phase::Recording { path, .. }, Event::Stopped) => Phase::Finalizing { path },
            (Phase::Finalizing { path }, Event::Captured(true)) => Phase::Delivering { path },
            (Phase::Finalizing { path }, Event::Captured(false)) => Phase::Failed { failure: Failure::Capture, path: Some(path) },
            (Phase::Delivering { path }, Event::Delivered(Ok(note))) => Phase::Delivered { path, note },
            (Phase::Delivering { path }, Event::Delivered(Err(words))) => {
                Phase::Failed { failure: Failure::Delivery(words), path: Some(path) }
            }
            (Phase::Delivered { .. } | Phase::Failed { .. }, Event::Dismiss) => Phase::Idle,
            (phase, _) => phase,
        }
    }

    pub fn recording(&self) -> bool {
        matches!(self, Phase::Recording { .. })
    }

    /// The file this phase names, for `Voir dans le Finder`.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Phase::Idle => None,
            Phase::Recording { path, .. } | Phase::Finalizing { path } | Phase::Delivering { path } | Phase::Delivered { path, .. } => {
                Some(path)
            }
            Phase::Failed { path, .. } => path.as_deref(),
        }
    }
}

// ---------------------------------------------------------------
// Commands
// ---------------------------------------------------------------

/// `montre-une-fois-20261106-1502.mov`, from the local date and time.
pub fn file_name(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> String {
    format!("montre-une-fois-{year:04}{month:02}{day:02}-{hour:02}{minute:02}.mov")
}

/// `dir/name`, or `dir/<stem>-2.mov`... when a recording of the same minute
/// is already there.
pub fn free_path(dir: &Path, name: &str, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    let first = dir.join(name);
    if !exists(&first) {
        return first;
    }
    let stem = name.trim_end_matches(".mov");
    (2..).map(|n| dir.join(format!("{stem}-{n}.mov"))).find(|p| !exists(p)).unwrap_or(first)
}

/// The screen video, the default input's audio, the clicks shown, no sound.
pub fn capture_argv(path: &Path) -> (String, Vec<String>) {
    (
        SCREENCAPTURE.to_string(),
        ["-v", "-g", "-k", "-x"].iter().map(|s| s.to_string()).chain(std::iter::once(path.to_string_lossy().into_owned())).collect(),
    )
}

/// `attn add` for the memo that carries the path, the path one argv element.
pub fn add_argv(add: &[String], path: &Path) -> Option<(String, Vec<String>)> {
    let (bin, words) = add.split_first()?;
    let mut args = words.to_vec();
    for w in [
        "--scope=pro",
        "--level=memo",
        "--kind=info",
        "--from=kova",
        "--topic=montre-une-fois",
        &format!("--title={ENTRY_TITLE}"),
        &format!("--body={}", path.display()),
    ] {
        args.push(w.to_string());
    }
    Some((bin.clone(), args))
}

/// What claap-agent is asked to do with the file.
pub fn instruction(path: &Path) -> String {
    format!(
        "Lance le skill montre-une-fois sur la vidéo {} (vidéo locale : images avec ffmpeg, voix avec whisper-cli, voir le skill)",
        path.display()
    )
}

/// `attn act <id> instruct --text=<instruction>`, the way a card's
/// instruction goes; `None` for an id that is not one.
pub fn instruct_argv(act: &[String], id: &str, path: &Path) -> Option<(String, Vec<String>)> {
    if !crate::attention::valid_id(id) {
        return None;
    }
    let (bin, words) = act.split_first()?;
    let mut args = words.to_vec();
    args.extend([id.to_string(), "instruct".to_string(), format!("--text={}", instruction(path))]);
    Some((bin.clone(), args))
}

/// The id `attn add` prints: its first word.
pub fn added_id(stdout: &str) -> Option<String> {
    stdout.split_whitespace().next().filter(|w| crate::attention::valid_id(w)).map(str::to_string)
}

/// Whether the file is done: non-empty and the same size on the last two
/// looks.
pub fn settled(sizes: &[u64]) -> bool {
    matches!(sizes, [.., a, b] if a == b && *b > 0)
}

/// `1:05`, `12:30`, `1:02:03`.
pub fn elapsed(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

// ---------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

// Feather `disc` (record) and `square` (stop).
const ICON_REC: &str = r#"<svg class="i" width="14" height="14" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="3"/></svg>"#;
const ICON_STOP: &str = r#"<svg class="i" width="12" height="12" viewBox="0 0 24 24"><rect x="5" y="5" width="14" height="14" rx="2"/></svg>"#;
const ICON_X: &str = r#"<svg class="i" width="14" height="14" viewBox="0 0 24 24"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>"#;

/// The header's button: `Enregistrer`, red `Arrêter` with the time while
/// recording, greyed while the file is finished and handed over.
pub fn button_html(phase: &Phase, now_ms: u64) -> String {
    match phase {
        Phase::Recording { started_ms, .. } => format!(
            "<button type=\"button\" class=\"btn rec on\" data-rec=\"stop\">{ICON_STOP}<span>Arrêter</span><span class=\"tm\">{}</span></button>",
            elapsed(now_ms.saturating_sub(*started_ms))
        ),
        Phase::Finalizing { .. } | Phase::Delivering { .. } => {
            format!("<button type=\"button\" class=\"btn sec rec\" disabled>{ICON_REC}<span>Enregistrer</span></button>")
        }
        _ => format!("<button type=\"button\" class=\"btn sec rec\" data-rec=\"start\">{ICON_REC}<span>Enregistrer</span></button>"),
    }
}

/// The line under the title once a recording stopped; nothing before.
pub fn note_html(phase: &Phase) -> String {
    let reveal = |p: &Path| {
        format!(
            "<button type=\"button\" class=\"link\" data-rec=\"reveal\" title=\"Voir dans le Finder\">{}</button>",
            esc(&p.display().to_string())
        )
    };
    let close = format!("<button type=\"button\" class=\"ico\" data-rec=\"dismiss\" title=\"Fermer\" aria-label=\"Fermer\">{ICON_X}</button>");
    let (class, body) = match phase {
        Phase::Idle | Phase::Recording { .. } => return String::new(),
        Phase::Finalizing { .. } => ("", "<div class=\"rt\">Finalisation de l'enregistrement\u{2026}</div>".to_string()),
        Phase::Delivering { path } => ("", format!("<div class=\"rt\">Remise à {CHIEF}\u{2026}</div>{}", reveal(path))),
        Phase::Delivered { path, note } => {
            let note = if note.trim().is_empty() { String::new() } else { format!("<span class=\"dim\">{}</span>", esc(note.trim())) };
            (" ok", format!("<div class=\"rt\">Enregistrement remis à {CHIEF}</div>{}{note}", reveal(path)))
        }
        Phase::Failed { failure: Failure::Capture, .. } => (" crit", format!("<div class=\"rt\">{}</div>", esc(PERMISSION_LINE))),
        Phase::Failed { failure: Failure::Delivery(words), path } => (
            " crit",
            format!(
                "<div class=\"rt\">Enregistrement non remis à {CHIEF} : {}</div>{}",
                esc(words),
                path.as_deref().map(reveal).unwrap_or_default()
            ),
        ),
    };
    let close = if matches!(phase, Phase::Delivered { .. } | Phase::Failed { .. }) { close } else { String::new() };
    format!("<div class=\"recn{class}\"><div class=\"rb\">{body}</div>{close}</div>")
}

// ---------------------------------------------------------------
// The one recording of the app
// ---------------------------------------------------------------

struct Recorder {
    phase: Phase,
    child: Option<Child>,
    /// `attn add` and `attn act`, taken at the click.
    add: Vec<String>,
    act: Vec<String>,
}

static REC: Mutex<Recorder> = Mutex::new(Recorder { phase: Phase::Idle, child: None, add: Vec::new(), act: Vec::new() });

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn apply(event: Event) {
    let mut rec = REC.lock();
    let phase = std::mem::take(&mut rec.phase);
    rec.phase = phase.next(event);
}

pub fn phase() -> Phase {
    REC.lock().phase.clone()
}

pub fn is_recording() -> bool {
    REC.lock().phase.recording()
}

/// The click on `Enregistrer`: screencapture starts on the main display.
/// `add` and `act` are the hand-off's commands.
pub fn start(add: Vec<String>, act: Vec<String>) {
    let mut rec = REC.lock();
    if matches!(rec.phase, Phase::Recording { .. } | Phase::Finalizing { .. } | Phase::Delivering { .. }) {
        return;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let dir = home.join("Movies").join("Kova");
    let w = crate::routines::When::now();
    let path = free_path(&dir, &file_name(w.year, w.month, w.day, w.hour, w.minute), &|p: &Path| p.exists());
    let (bin, args) = capture_argv(&path);
    let spawned = std::fs::create_dir_all(&dir).map_err(|e| e.to_string()).and_then(|()| {
        std::process::Command::new(&bin)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())
    });
    let phase = std::mem::take(&mut rec.phase);
    match spawned {
        Ok(child) => {
            log::info!("record: screencapture {} started (pid {})", path.display(), child.id());
            rec.child = Some(child);
            rec.add = add;
            rec.act = act;
            rec.phase = phase.next(Event::Started { path, at_ms: now_ms() });
        }
        Err(e) => {
            log::warn!("record: cannot start screencapture: {e}");
            rec.phase = phase.next(Event::StartFailed);
        }
    }
}

/// The click on `Arrêter`: SIGINT, then the file is finished on a thread.
pub fn stop() {
    let mut rec = REC.lock();
    if !rec.phase.recording() {
        return;
    }
    let child = rec.child.take();
    if let Some(c) = &child {
        unsafe { libc::kill(c.id() as libc::pid_t, libc::SIGINT) };
    }
    let phase = std::mem::take(&mut rec.phase);
    rec.phase = phase.next(Event::Stopped);
    drop(rec);
    finish(child);
}

/// Once per tick: screencapture that ended on its own (a refused
/// permission, an error) is finished like a stop.
pub fn poll() {
    let mut rec = REC.lock();
    if !rec.phase.recording() {
        return;
    }
    let exited = rec.child.as_mut().is_none_or(|c| matches!(c.try_wait(), Ok(Some(_)) | Err(_)));
    if !exited {
        return;
    }
    log::warn!("record: screencapture ended on its own");
    let child = rec.child.take();
    let phase = std::mem::take(&mut rec.phase);
    rec.phase = phase.next(Event::Stopped);
    drop(rec);
    finish(child);
}

/// Close the note under the title.
pub fn dismiss() {
    apply(Event::Dismiss);
}

/// The file `Voir dans le Finder` shows: the one this recording made.
pub fn reveal() {
    let Some(path) = phase().path().map(Path::to_path_buf) else { return };
    let _ = std::process::Command::new("/usr/bin/open").arg("-R").arg(&path).spawn();
}

/// Kova quits: a running screencapture is stopped so it does not record on
/// alone, and gets a moment to write its file.
pub fn abort() {
    let mut rec = REC.lock();
    if let Some(mut c) = rec.child.take() {
        unsafe { libc::kill(c.id() as libc::pid_t, libc::SIGINT) };
        for _ in 0..30 {
            if matches!(c.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

/// Wait for screencapture to exit and the file to settle, then hand it over.
fn finish(child: Option<Child>) {
    std::thread::spawn(move || {
        if let Some(mut c) = child {
            // It writes the movie's index after the signal; a minute at most.
            let mut done = false;
            for _ in 0..600 {
                if matches!(c.try_wait(), Ok(Some(_)) | Err(_)) {
                    done = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if !done {
                log::warn!("record: screencapture did not exit, killing it");
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        let Some(path) = phase().path().map(Path::to_path_buf) else { return };
        let mut sizes = Vec::new();
        for _ in 0..20 {
            sizes.push(std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0));
            if settled(&sizes) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        let ok = settled(&sizes);
        if !ok {
            log::warn!("record: {} is missing or empty", path.display());
        }
        apply(Event::Captured(ok));
        if ok {
            let (add, act) = {
                let rec = REC.lock();
                (rec.add.clone(), rec.act.clone())
            };
            apply(Event::Delivered(deliver(&add, &act, &path)));
        }
    });
}

/// `attn add`, then `attn act <id> instruct`: the chief gets the file.
fn deliver(add: &[String], act: &[String], path: &Path) -> Result<String, String> {
    let run = |(bin, args): (String, Vec<String>), secs: u64| -> Result<String, String> {
        let out = crate::exec::run_with_deadline(std::process::Command::new(&bin).args(&args), std::time::Duration::from_secs(secs))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            let line: String = err.lines().next().unwrap_or("").trim().chars().take(120).collect();
            Err(if line.is_empty() { format!("{bin} exited with {}", out.status) } else { line })
        }
    };
    let added = run(add_argv(add, path).ok_or("pas de commande attn")?, 20)?;
    let id = added_id(&added).ok_or_else(|| format!("réponse d'attn add illisible : {added}"))?;
    let said = run(instruct_argv(act, &id, path).ok_or("pas de commande attn act")?, 90)?;
    log::info!("record: {} handed to {CHIEF} as {id}: {said}", path.display());
    Ok(said.lines().next().unwrap_or_default().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn the_phases_go_idle_recording_finalizing_delivered() {
        let path = p("/Users/r/Movies/Kova/a b.mov");
        let rec = Phase::Idle.next(Event::Started { path: path.clone(), at_ms: 5 });
        assert_eq!(rec, Phase::Recording { path: path.clone(), started_ms: 5 });
        assert!(rec.recording());
        // A second start while recording changes nothing.
        assert_eq!(rec.clone().next(Event::Started { path: p("/x"), at_ms: 9 }), rec);
        let fin = rec.next(Event::Stopped);
        assert_eq!(fin, Phase::Finalizing { path: path.clone() });
        assert_eq!(fin.clone().next(Event::Stopped), fin, "a second stop is nothing");
        let del = fin.next(Event::Captured(true));
        assert_eq!(del, Phase::Delivering { path: path.clone() });
        let done = del.next(Event::Delivered(Ok("remis".into())));
        assert_eq!(done, Phase::Delivered { path: path.clone(), note: "remis".into() });
        assert_eq!(done.path(), Some(path.as_path()));
        assert_eq!(done.clone().next(Event::Dismiss), Phase::Idle);
        // A new recording may start from a finished one.
        assert!(done.next(Event::Started { path: p("/y"), at_ms: 1 }).recording());
    }

    #[test]
    fn an_empty_file_or_no_start_is_a_capture_failure_and_a_cli_error_a_delivery_one() {
        let path = p("/m.mov");
        let fin = Phase::Finalizing { path: path.clone() };
        assert_eq!(fin.next(Event::Captured(false)), Phase::Failed { failure: Failure::Capture, path: Some(path.clone()) });
        assert_eq!(Phase::Idle.next(Event::StartFailed), Phase::Failed { failure: Failure::Capture, path: None });
        let del = Phase::Delivering { path: path.clone() };
        assert_eq!(
            del.next(Event::Delivered(Err("boom".into()))),
            Phase::Failed { failure: Failure::Delivery("boom".into()), path: Some(path) }
        );
        // Idle ignores what belongs to a recording.
        for e in [Event::Stopped, Event::Captured(true), Event::Delivered(Ok(String::new())), Event::Dismiss] {
            assert_eq!(Phase::Idle.next(e), Phase::Idle);
        }
        // A recording is never dismissed.
        let rec = Phase::Recording { path: p("/r"), started_ms: 0 };
        assert_eq!(rec.clone().next(Event::Dismiss), rec);
    }

    #[test]
    fn the_commands_keep_a_path_with_spaces_as_one_argument() {
        let path = p("/Users/robin bonduelle/Movies/Kova/montre-une-fois-20261106-1502.mov");
        let (bin, args) = capture_argv(&path);
        assert_eq!(bin, "/usr/sbin/screencapture");
        assert_eq!(args, ["-v", "-g", "-k", "-x", "/Users/robin bonduelle/Movies/Kova/montre-une-fois-20261106-1502.mov"]);
        let attn = vec!["/Users/x y/.local/bin/attn".to_string(), "add".to_string()];
        let (bin, args) = add_argv(&attn, &path).unwrap();
        assert_eq!(bin, "/Users/x y/.local/bin/attn");
        assert_eq!(args.first().map(String::as_str), Some("add"));
        assert!(args.contains(&"--level=memo".to_string()) && args.contains(&"--kind=info".to_string()));
        assert!(args.contains(&"--from=kova".to_string()) && args.contains(&"--topic=montre-une-fois".to_string()));
        assert!(args.contains(&"--title=Enregistrement à transformer en skill".to_string()));
        assert_eq!(args.last().unwrap(), &format!("--body={}", path.display()));
        let act = vec!["/bin/attn".to_string(), "act".to_string()];
        let (_, args) = instruct_argv(&act, "a-1b2c3d", &path).unwrap();
        assert_eq!(args[..3], ["act", "a-1b2c3d", "instruct"]);
        assert_eq!(args.len(), 4);
        assert!(args[3].starts_with("--text=Lance le skill montre-une-fois sur la vidéo /Users/robin bonduelle/Movies/"));
        assert!(args[3].ends_with("(vidéo locale : images avec ffmpeg, voix avec whisper-cli, voir le skill)"));
        assert_eq!(instruct_argv(&act, "-rf", &path), None, "an id is never an option");
        assert_eq!(add_argv(&[], &path), None);
        assert_eq!(added_id("a-1b2c3d\n").as_deref(), Some("a-1b2c3d"));
        assert_eq!(added_id(""), None);
    }

    #[test]
    fn the_file_is_named_by_the_minute_and_never_overwritten() {
        assert_eq!(file_name(2026, 11, 6, 9, 5), "montre-une-fois-20261106-0905.mov");
        let dir = p("/m");
        let taken = |q: &Path| q == Path::new("/m/a.mov") || q == Path::new("/m/a-2.mov");
        assert_eq!(free_path(&dir, "b.mov", &taken), p("/m/b.mov"));
        assert_eq!(free_path(&dir, "a.mov", &taken), p("/m/a-3.mov"));
    }

    #[test]
    fn the_file_is_done_once_its_size_holds_and_is_not_zero() {
        assert!(!settled(&[]));
        assert!(!settled(&[10]));
        assert!(!settled(&[10, 20]));
        assert!(!settled(&[0, 0]));
        assert!(settled(&[10, 20, 20]));
        assert_eq!(elapsed(65_000), "1:05");
        assert_eq!(elapsed(3_723_000), "1:02:03");
    }

    #[test]
    fn the_button_and_the_note_follow_the_phase() {
        assert!(button_html(&Phase::Idle, 0).contains("data-rec=\"start\"") && button_html(&Phase::Idle, 0).contains("Enregistrer"));
        let rec = Phase::Recording { path: p("/a.mov"), started_ms: 1_000 };
        let live = button_html(&rec, 66_000);
        assert!(live.contains("data-rec=\"stop\"") && live.contains("Arrêter") && live.contains("1:05") && live.contains("rec on"));
        assert!(button_html(&Phase::Finalizing { path: p("/a") }, 0).contains("disabled"));
        assert_eq!(note_html(&Phase::Idle), "");
        assert_eq!(note_html(&rec), "");
        let done = note_html(&Phase::Delivered { path: p("/Users/r/Movies/Kova/<a>.mov"), note: "remis".into() });
        assert!(done.contains("Enregistrement remis à claap-agent"));
        assert!(done.contains("data-rec=\"reveal\"") && done.contains("/Users/r/Movies/Kova/&lt;a&gt;.mov"));
        assert!(done.contains("data-rec=\"dismiss\""));
        let failed = note_html(&Phase::Failed { failure: Failure::Capture, path: None });
        assert!(failed.contains(&esc(PERMISSION_LINE)) && failed.contains("recn crit"));
        let lost = note_html(&Phase::Failed { failure: Failure::Delivery("x <y>".into()), path: Some(p("/a.mov")) });
        assert!(lost.contains("non remis à claap-agent : x &lt;y&gt;") && lost.contains("data-rec=\"reveal\""));
    }
}
