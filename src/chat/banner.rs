//! The turn's state, shown above the chat's box so the user knows whether
//! a message leaves now: `Travaille · 1 min 12 s`, `Attend ta réponse`,
//! `Prêt`, `Hors ligne` (KovaLink's readability rule 1).
//!
//! Driven by the session's own events, never by a timer guessing:
//! - Claude Code's busy signal, pushed through the PTY (`Pane::is_working`:
//!   its title spinner, or `working` in OSC 7501);
//! - the session's hooks (`~/.claude/state/turns/<session id>`, written by
//!   `claude-config/hooks/harness.py` on UserPromptSubmit and Stop, by
//!   `notify.sh` on a permission prompt or a question), read when the file
//!   changes;
//! - the transcript: an `AskUserQuestion` or `ExitPlanMode` call with no
//!   result yet is a question on screen;
//! - the cards (a permission prompt read on screen, a `kova ask` of the pane).

use std::path::{Path, PathBuf};

/// What a hook of the session said last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    Working,
    Waiting,
    Ready,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HookTurn {
    pub turn: Turn,
    /// Epoch milliseconds of the hook.
    pub at_ms: u64,
}

/// Stop ran and the pane did not turn busy since: the turn is over, whatever
/// the pane still says. Claude Code keeps reporting `working` (OSC 7501) after
/// its turn while background agents run (`✻ Waiting for 2 background agents
/// to finish`), and its prompt takes a message then (measured on 2.1.319). A
/// busy pane that came on after the Stop is a new turn, its prompt hook not
/// read yet.
pub fn turn_ended(hook: Option<HookTurn>, spinner_on_at: Option<u64>) -> bool {
    hook.is_some_and(|h| h.turn == Turn::Ready && spinner_on_at.is_none_or(|on| on <= h.at_ms))
}

/// A prompt just submitted counts as working this long before the spinner
/// shows; past it, the spinner alone says so (a turn interrupted with Esc
/// never runs the Stop hook).
pub const HOOK_BRIDGE_MS: u64 = 5_000;

/// Where the hooks write the turn of `session_id`: under `~/.claude` even
/// for a session with its own config dir (the hook writes under `$HOME`).
pub fn hook_path(home: &Path, session_id: &str) -> Option<PathBuf> {
    crate::claude_session::is_safe_session_id(session_id).then(|| home.join(".claude/state/turns").join(session_id))
}

/// `working 1762430000.123`: the state, then epoch seconds.
pub fn parse_hook(text: &str) -> Option<HookTurn> {
    let mut words = text.split_whitespace();
    let turn = match words.next()? {
        "working" => Turn::Working,
        "waiting" => Turn::Waiting,
        "ready" => Turn::Ready,
        _ => return None,
    };
    let secs: f64 = words.next()?.parse().ok()?;
    (secs.is_finite() && secs > 0.0).then(|| HookTurn { turn, at_ms: (secs * 1000.0) as u64 })
}

/// What the pane, its hooks and its transcript say, read on the tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BannerInput {
    /// A Claude session runs in the pane.
    pub has_session: bool,
    /// Claude Code says it is busy: its title spinner, or `working` in OSC
    /// 7501 (`Pane::is_working`).
    pub spinner: bool,
    /// When the chat saw it turn busy, epoch ms (`None` when it already was).
    pub spinner_on_at: Option<u64>,
    /// When the spinner last went off, epoch ms.
    pub spinner_off_at: Option<u64>,
    /// A card waits: a permission prompt on screen, a `kova ask` of the pane.
    pub cards: bool,
    /// The transcript ends on a question to the user (`AskUserQuestion`,
    /// `ExitPlanMode`) still unanswered.
    pub question: bool,
    /// Claude Code's `❯` line is on screen, empty or not: no dialog over it.
    pub prompt_line: bool,
    pub hook: Option<HookTurn>,
    pub now_ms: u64,
    /// When the turn in flight started, epoch seconds (the user's last
    /// message, else the moment the chat saw it working).
    pub working_since: Option<u64>,
    /// When the agent last did something, epoch seconds.
    pub last_activity: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Banner {
    Working { since: Option<u64> },
    Waiting,
    Done { at: Option<u64> },
    Offline,
}

/// The state for what the pane says. A question waiting wins over the
/// spinner: a permission prompt keeps the title of a working Claude for a
/// moment, and the answer is what the user must give. A hook's `waiting`
/// holds only while a dialog covers the prompt line and the spinner is off:
/// answered, or dismissed with Esc, it is over.
pub fn banner(input: BannerInput) -> Banner {
    if !input.has_session {
        return Banner::Offline;
    }
    let hook = |turn| input.hook.filter(|h| h.turn == turn);
    let asked = input.question || hook(Turn::Waiting).is_some();
    if input.cards || (asked && !input.spinner && !input.prompt_line) {
        return Banner::Waiting;
    }
    let working = (input.spinner && !turn_ended(input.hook, input.spinner_on_at))
        || hook(Turn::Working).is_some_and(|h| {
            input.now_ms < h.at_ms + HOOK_BRIDGE_MS && input.spinner_off_at.is_none_or(|off| off < h.at_ms)
        });
    if working {
        return Banner::Working { since: input.working_since };
    }
    Banner::Done { at: input.last_activity }
}

impl Banner {
    /// The page's class for the state: its colour and its animation.
    pub fn name(self) -> &'static str {
        match self {
            Banner::Working { .. } => "working",
            Banner::Waiting => "waiting",
            Banner::Done { .. } => "done",
            Banner::Offline => "offline",
        }
    }
}

/// `1 min 12 s`, `45 s`, `2 h 03 min`: the duration the banner shows.
pub fn duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min {} s", s / 60, s % 60),
        s => format!("{} h {:02} min", s / 3600, (s % 3600) / 60),
    }
}

/// `à l'instant`, `il y a 3 min`, `il y a 2 h`, `il y a 3 j`.
pub fn ago(secs: u64) -> String {
    match secs {
        s if s < 60 => "à l'instant".to_string(),
        s if s < 3600 => format!("il y a {} min", s / 60),
        s if s < 86_400 => format!("il y a {} h", s / 3600),
        s => format!("il y a {} j", s / 86_400),
    }
}

/// The banner's text at `now`. The tick sends it again when it changes, so
/// the clock moves once a second while the agent works.
pub fn text(banner: Banner, now: u64) -> String {
    match banner {
        Banner::Working { since: Some(t) } => format!("Travaille · {}", duration(now.saturating_sub(t))),
        Banner::Working { since: None } => "Travaille".to_string(),
        Banner::Waiting => "Attend ta réponse".to_string(),
        Banner::Done { at: Some(t) } => format!("Prêt · fini {}", ago(now.saturating_sub(t))),
        Banner::Done { at: None } => "Prêt".to_string(),
        Banner::Offline => "Hors ligne".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> BannerInput {
        BannerInput { has_session: true, prompt_line: true, now_ms: 100_000, ..Default::default() }
    }

    fn hook(turn: Turn, at_ms: u64) -> Option<HookTurn> {
        Some(HookTurn { turn, at_ms })
    }

    #[test]
    fn no_session_is_offline_whatever_else_holds() {
        assert_eq!(banner(BannerInput { spinner: true, cards: true, ..Default::default() }), Banner::Offline);
    }

    #[test]
    fn a_question_wins_over_the_spinner() {
        assert_eq!(banner(BannerInput { spinner: true, cards: true, ..input() }), Banner::Waiting);
    }

    #[test]
    fn the_hook_file_is_read() {
        assert_eq!(parse_hook("working 1762430000.123\n"), hook(Turn::Working, 1_762_430_000_123));
        assert_eq!(parse_hook("waiting 5"), hook(Turn::Waiting, 5000));
        assert_eq!(parse_hook("ready 5"), hook(Turn::Ready, 5000));
        assert_eq!(parse_hook("busy 5"), None);
        assert_eq!(parse_hook("ready"), None);
        assert_eq!(parse_hook(""), None);
        assert_eq!(hook_path(Path::new("/h"), "../x"), None);
        assert_eq!(hook_path(Path::new("/h"), "abc-1"), Some(PathBuf::from("/h/.claude/state/turns/abc-1")));
    }

    #[test]
    fn a_submitted_prompt_is_working_before_the_spinner_shows() {
        let now = input().now_ms;
        assert_eq!(banner(BannerInput { hook: hook(Turn::Working, now - 100), ..input() }), Banner::Working { since: None });
        // The spinner came and went after it: the turn is over (Esc, no Stop hook).
        let after = BannerInput { hook: hook(Turn::Working, now - 3000), spinner_off_at: Some(now - 1000), ..input() };
        assert_eq!(banner(after), Banner::Done { at: None });
        // Too old to bridge anything.
        assert_eq!(banner(BannerInput { hook: hook(Turn::Working, now - HOOK_BRIDGE_MS), ..input() }), Banner::Done { at: None });
        assert_eq!(banner(BannerInput { spinner: true, hook: hook(Turn::Working, now - 60_000), ..input() }), Banner::Working { since: None }, "the spinner holds it");
    }

    #[test]
    fn waiting_for_background_agents_is_ready() {
        // Measured on Claude Code 2.1.319: the turn ends on `launched2`, Stop
        // writes `ready`, then the screen says `✻ Waiting for 1 background
        // agent to finish` and OSC 7501 says `working` until the agent is done.
        let stop = parse_hook("ready 1793979027.648\n");
        let busy = BannerInput { spinner: true, spinner_on_at: Some(1_793_979_002_000), hook: stop, now_ms: 1_793_979_028_500, ..input() };
        assert_eq!(banner(BannerInput { last_activity: Some(1_793_979_027), ..busy }), Banner::Done { at: Some(1_793_979_027) });
        assert_eq!(banner(BannerInput { spinner_on_at: None, ..busy }), Banner::Done { at: None }, "busy since before the chat looked");
        // Busy again after the Stop: a new turn, its prompt hook not read yet.
        assert_eq!(banner(BannerInput { spinner_on_at: Some(1_793_979_030_000), ..busy }), Banner::Working { since: None });
        // The agent's result starts a turn of its own: UserPromptSubmit says so.
        let next = BannerInput { hook: parse_hook("working 1793979069.330\n"), now_ms: 1_793_979_070_000, ..busy };
        assert_eq!(banner(next), Banner::Working { since: None });
        assert_eq!(banner(BannerInput { spinner: false, ..next }), Banner::Working { since: None }, "the hook alone, within the bridge");
    }

    #[test]
    fn a_question_waits_while_its_dialog_is_on_screen() {
        let now = input().now_ms;
        let dialog = BannerInput { prompt_line: false, ..input() };
        assert_eq!(banner(BannerInput { hook: hook(Turn::Waiting, now - 60_000), ..dialog }), Banner::Waiting, "however long ago");
        assert_eq!(banner(BannerInput { question: true, ..dialog }), Banner::Waiting, "AskUserQuestion, from the transcript");
        // Answered: Claude works again, or the dialog left the screen.
        assert_eq!(banner(BannerInput { hook: hook(Turn::Waiting, now), spinner: true, ..dialog }), Banner::Working { since: None });
        assert_eq!(banner(BannerInput { hook: hook(Turn::Waiting, now), ..input() }), Banner::Done { at: None });
        assert_eq!(banner(BannerInput { question: true, ..input() }), Banner::Done { at: None });
    }

    #[test]
    fn working_counts_from_the_turn_start() {
        let b = banner(BannerInput { spinner: true, working_since: Some(1000), ..input() });
        assert_eq!(text(b, 1072), "Travaille · 1 min 12 s");
        assert_eq!(text(Banner::Working { since: Some(0) }, 7380), "Travaille · 2 h 03 min");
    }

    #[test]
    fn idle_says_it_is_ready() {
        let b = banner(BannerInput { last_activity: Some(1000), ..input() });
        assert_eq!(text(b, 1000 + 180), "Prêt · fini il y a 3 min");
        assert_eq!(text(b, 1010), "Prêt · fini à l'instant");
        assert_eq!(text(Banner::Done { at: None }, 5), "Prêt");
        assert_eq!(text(Banner::Waiting, 5), "Attend ta réponse");
        assert_eq!(text(Banner::Offline, 5), "Hors ligne");
    }

    #[test]
    fn each_state_has_its_class_on_the_page() {
        let names: Vec<&str> =
            [Banner::Working { since: None }, Banner::Waiting, Banner::Done { at: None }, Banner::Offline].map(Banner::name).to_vec();
        assert_eq!(names, ["working", "waiting", "done", "offline"]);
    }
}
