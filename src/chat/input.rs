//! The chat's input box: what may be typed into the pane's PTY, and when.
//!
//! The text goes to Claude Code as typed text then Enter, only while its
//! prompt is idle and empty: never into a permission prompt, never on top of
//! what the user typed in the terminal, never mid-turn. A message sent while
//! the agent works waits in the box's queue (shown, cancellable) and leaves
//! once the prompt is free.
//!
//! The text is the user's, typed in Kova's own box, but it lands in a
//! terminal: every control character is removed (an ESC would start a
//! sequence, a ^U would erase the line), and it is pasted, bracketed when
//! the app asked for it, so a newline inside is part of the message, not an
//! Enter. Enter goes on its own, a moment later: Claude Code takes a pasted
//! newline as part of the message (the same reason `ask::deliver_late` waits).

/// Longest message accepted, the cap of `answer-ask` (`ask::ANSWER_TEXT_MAX`).
pub const TEXT_MAX: usize = 8 * 1024;
/// Between the pasted text and its Enter.
pub const ENTER_DELAY_MS: u64 = 300;
/// An Enter not written this long after it was due (the pane was not shown
/// as a chat meanwhile) is dropped: the screen it was meant for is gone.
pub const ENTER_STALE_MS: u64 = 2_000;

/// What Claude Code's input line shows, read off the pane's screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Composer {
    Empty,
    /// Something sits in it: the user's own draft, or a collapsed paste.
    Typed,
    /// No `❯` line on screen: a dialog, a menu, a shell.
    Unreadable,
}

/// The composer starts with this character (KovaLink's `COMPOSER_PREFIX`).
const COMPOSER_PREFIX: char = '❯';
/// Lines under the `❯` that still belong to the input: a multi-line paste
/// shows as `[Pasted text #3 +11 lines]` below it.
const COMPOSER_TAIL_LINES: usize = 6;
const PASTED_PREFIX: &str = "[Pasted text";
/// Claude Code's hint in an empty composer at the start of a session.
const PLACEHOLDER_PREFIX: &str = "Try \"";

/// The composer's state on `visible` (the pane's screen as text), on
/// KovaLink's rule (`keygate.ts`, `composerIsEmpty`): the last line that
/// starts with `❯`, empty after it, no collapsed paste below. One more rule
/// than KovaLink's: the `❯` must sit in the first column. A menu or a
/// dialog (a permission prompt's `❯ 1. Yes`, the trust dialog's
/// `❯ No, exit`) indents its marker, and is no composer to type into.
pub fn composer(visible: &str) -> Composer {
    let lines: Vec<&str> = visible.lines().collect();
    let Some(i) = lines.iter().rposition(|l| l.trim_start().starts_with(COMPOSER_PREFIX)) else {
        return Composer::Unreadable;
    };
    let line = lines[i];
    if !line.starts_with(COMPOSER_PREFIX) {
        return Composer::Unreadable;
    }
    let after = line[COMPOSER_PREFIX.len_utf8()..]
        .trim_end_matches(|c: char| c.is_whitespace() || ('\u{2500}'..='\u{257f}').contains(&c))
        .trim();
    if !after.is_empty() && !after.starts_with(PLACEHOLDER_PREFIX) {
        return Composer::Typed;
    }
    let tail = &lines[i + 1..lines.len().min(i + COMPOSER_TAIL_LINES)];
    let pasted = tail.iter().any(|l| l.contains(PASTED_PREFIX));
    // A draft that starts with a new line: text under the `❯`, above the
    // composer's bottom rule.
    let below = tail.iter().position(|l| l.trim_start().starts_with('\u{2500}')).is_some_and(|end| tail[..end].iter().any(|l| !l.trim().is_empty()));
    if pasted || below { Composer::Typed } else { Composer::Empty }
}

/// What the pane says, for the gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateInput {
    pub has_session: bool,
    pub working: bool,
    pub permission_prompt: bool,
    pub composer: Composer,
}

/// Whether a message may be typed now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Send,
    /// Not now, later: the message waits in the queue.
    Wait(&'static str),
    /// Not at all: the message goes back to the box.
    Refuse(&'static str),
}

pub fn gate(input: GateInput) -> Verdict {
    if !input.has_session {
        return Verdict::Refuse("Claude is not running in this pane");
    }
    if input.permission_prompt {
        return Verdict::Wait("Sends once the prompt is answered");
    }
    if input.working {
        return Verdict::Wait("Sends when Claude is idle");
    }
    match input.composer {
        Composer::Empty => Verdict::Send,
        Composer::Typed => Verdict::Refuse("The terminal's prompt already holds text: send or clear it there"),
        Composer::Unreadable => Verdict::Wait("Sends once Claude's prompt is back"),
    }
}

/// The message as it may reach a terminal: CRs turned into newlines, every
/// other control character (C0 but tab and newline, DEL, C1) dropped,
/// trimmed. An error says why nothing is sent.
pub fn sanitize(raw: &str) -> Result<String, &'static str> {
    let text: String = raw
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|&c| c == '\n' || c == '\t' || !(c.is_control()))
        .collect();
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Nothing to send");
    }
    if text.len() > TEXT_MAX {
        return Err("Too long: 8 KB at most");
    }
    Ok(text)
}

/// The text to paste: as is inside a bracketed paste; on one line when the
/// app did not ask for one, since each newline would be an Enter.
pub fn typed(text: &str, bracketed: bool) -> String {
    if bracketed { text.to_string() } else { text.replace('\n', " ") }
}

/// The box's one message in flight: waiting in the queue, or typed and
/// waiting for its Enter. Times are milliseconds on any monotonic clock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outbox {
    queued: Option<String>,
    enter_due: Option<u64>,
}

/// What the caller does now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Nothing,
    /// Paste this into the PTY; Enter follows from `poll`.
    Type(String),
    Enter,
    /// The Enter came too late and was dropped: the text waits in the
    /// terminal's prompt.
    Stale,
    /// Kept in the queue, for this reason.
    Queued(&'static str),
    /// Not sent: the text goes back to the box.
    Refused { reason: &'static str, text: String },
}

impl Outbox {
    pub fn queued(&self) -> Option<&str> {
        self.queued.as_deref()
    }

    /// The user pressed Enter in the box with `text` (already sanitized).
    pub fn submit(&mut self, text: String, verdict: Verdict, now: u64) -> Step {
        if self.queued.is_some() || self.enter_due.is_some() {
            return Step::Refused { reason: "A message is already on its way", text };
        }
        match verdict {
            Verdict::Send => {
                self.enter_due = Some(now + ENTER_DELAY_MS);
                Step::Type(text)
            }
            Verdict::Wait(reason) => {
                self.queued = Some(text);
                Step::Queued(reason)
            }
            Verdict::Refuse(reason) => Step::Refused { reason, text },
        }
    }

    /// On the tick: the Enter of a typed message once its delay ran out, or
    /// the queued message once the gate opens (or back to the box once it
    /// refuses).
    pub fn poll(&mut self, verdict: Verdict, now: u64) -> Step {
        if let Some(due) = self.enter_due {
            if now < due {
                return Step::Nothing;
            }
            self.enter_due = None;
            return if now - due > ENTER_STALE_MS { Step::Stale } else { Step::Enter };
        }
        match (verdict, self.queued.is_some()) {
            (_, false) | (Verdict::Wait(_), true) => Step::Nothing,
            (Verdict::Send, true) => {
                self.enter_due = Some(now + ENTER_DELAY_MS);
                Step::Type(self.queued.take().unwrap_or_default())
            }
            (Verdict::Refuse(reason), true) => Step::Refused { reason, text: self.queued.take().unwrap_or_default() },
        }
    }

    /// The queue's Cancel: the text goes back to the box.
    pub fn cancel(&mut self) -> Option<String> {
        self.queued.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle() -> GateInput {
        GateInput { has_session: true, working: false, permission_prompt: false, composer: Composer::Empty }
    }

    #[test]
    fn the_composer_is_read_off_the_screen() {
        assert_eq!(composer("output\n───\n❯ \n───\n  ? for shortcuts"), Composer::Empty);
        assert_eq!(composer("❯ fix the bug\n"), Composer::Typed);
        assert_eq!(composer("❯ \n  [Pasted text #2 +11 lines]"), Composer::Typed);
        assert_eq!(composer("───\n❯ \n  second line of a draft\n───\n  ? for shortcuts"), Composer::Typed, "a draft starting with a new line");
        assert_eq!(composer("❯ │"), Composer::Empty, "the frame's box drawing is not text");
        assert_eq!(composer("❯ \n Do you want to proceed?\n ❯ 1. Yes\n   2. No"), Composer::Unreadable, "a menu is no composer");
        assert_eq!(composer("❯ Try \"refactor <filepath>\""), Composer::Empty, "the start hint is not a draft");
        assert_eq!(composer("~/src $ "), Composer::Unreadable);
        // The last ❯ is the composer, not an echoed one above it.
        assert_eq!(composer("❯ earlier message\nanswer\n❯ "), Composer::Empty);
    }

    #[test]
    fn the_gate_sends_only_to_an_idle_empty_prompt() {
        assert_eq!(gate(idle()), Verdict::Send);
        assert!(matches!(gate(GateInput { working: true, ..idle() }), Verdict::Wait(_)));
        assert!(matches!(gate(GateInput { permission_prompt: true, ..idle() }), Verdict::Wait(_)));
        assert!(matches!(gate(GateInput { composer: Composer::Unreadable, ..idle() }), Verdict::Wait(_)));
        assert!(matches!(gate(GateInput { composer: Composer::Typed, ..idle() }), Verdict::Refuse(_)));
        assert!(matches!(gate(GateInput { has_session: false, ..idle() }), Verdict::Refuse(_)));
        // A prompt on screen wins over a draft: answering it comes first.
        assert!(matches!(gate(GateInput { permission_prompt: true, composer: Composer::Typed, ..idle() }), Verdict::Wait(_)));
    }

    #[test]
    fn control_characters_never_reach_the_terminal() {
        assert_eq!(sanitize("  a\x1b[31mb\x15c\x07\u{9b}d\r\ne\tf  ").unwrap(), "a[31mbcd\ne\tf");
        assert!(sanitize(" \n\x1b ").is_err());
        assert!(sanitize(&"x".repeat(TEXT_MAX + 1)).is_err());
        assert_eq!(typed("a\nb", true), "a\nb");
        assert_eq!(typed("a\nb", false), "a b", "without a bracketed paste a newline would be an Enter");
    }

    #[test]
    fn a_message_is_typed_then_entered_after_the_delay() {
        let mut out = Outbox::default();
        assert_eq!(out.submit("hi".into(), Verdict::Send, 1000), Step::Type("hi".into()));
        assert_eq!(out.poll(Verdict::Send, 1000 + ENTER_DELAY_MS - 1), Step::Nothing);
        assert!(matches!(out.submit("again".into(), Verdict::Send, 1100), Step::Refused { .. }), "one at a time");
        assert_eq!(out.poll(Verdict::Wait("busy"), 1000 + ENTER_DELAY_MS), Step::Enter, "the Enter of a typed text always follows");
        assert_eq!(out.poll(Verdict::Send, 2000), Step::Nothing);
    }

    #[test]
    fn a_late_enter_is_dropped() {
        let mut out = Outbox::default();
        out.submit("hi".into(), Verdict::Send, 0);
        assert_eq!(out.poll(Verdict::Send, ENTER_DELAY_MS + ENTER_STALE_MS + 1), Step::Stale);
        assert_eq!(out.poll(Verdict::Send, 60_000), Step::Nothing);
    }

    #[test]
    fn a_queued_message_leaves_when_the_prompt_frees_up() {
        let mut out = Outbox::default();
        assert_eq!(out.submit("next".into(), Verdict::Wait("busy"), 0), Step::Queued("busy"));
        assert_eq!(out.queued(), Some("next"));
        assert_eq!(out.poll(Verdict::Wait("busy"), 10), Step::Nothing);
        assert_eq!(out.poll(Verdict::Send, 20), Step::Type("next".into()));
        assert_eq!(out.poll(Verdict::Send, 20 + ENTER_DELAY_MS), Step::Enter);
    }

    #[test]
    fn a_queued_message_goes_back_to_the_box_when_refused_or_cancelled() {
        let mut out = Outbox::default();
        out.submit("next".into(), Verdict::Wait("busy"), 0);
        assert_eq!(out.poll(Verdict::Refuse("gone"), 5), Step::Refused { reason: "gone", text: "next".into() });
        out.submit("other".into(), Verdict::Wait("busy"), 0);
        assert_eq!(out.cancel().as_deref(), Some("other"));
        assert_eq!(out.queued(), None);
    }
}
