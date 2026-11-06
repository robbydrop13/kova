//! The chat's input box: what may be typed into the pane's PTY, and when.
//!
//! The text goes to Claude Code as typed text then Enter, only once its turn
//! is over: never into a permission prompt or a question, never mid-turn. A
//! message sent meanwhile waits in the box's queue (shown, editable,
//! cancellable) and the queue leaves in order, one message per turn. A draft
//! left in the terminal's prompt goes along with the message.
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
    /// The turn's state (`banner::status`): only `Ready` takes a message.
    pub status: super::banner::Banner,
    pub composer: Composer,
}

/// Whether a message may be typed now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Send,
    /// Not now, later: the message waits in the queue.
    Wait(&'static str),
}

/// A draft in the terminal's prompt does not bounce the message: the text is
/// pasted after it and both leave together (Claude Code's own grey prompt
/// suggestion reads as text too, and a paste replaces it).
pub fn gate(input: GateInput) -> Verdict {
    use super::banner::Banner;
    if !input.has_session {
        return Verdict::Wait("part quand Claude tourne dans ce pane");
    }
    match input.status {
        Banner::Waiting => return Verdict::Wait("part une fois la question répondue"),
        Banner::Working { .. } => return Verdict::Wait("part à la fin du tour"),
        Banner::Offline => return Verdict::Wait("part quand Claude tourne dans ce pane"),
        Banner::Done { .. } => {}
    }
    match input.composer {
        Composer::Empty | Composer::Typed => Verdict::Send,
        Composer::Unreadable => Verdict::Wait("part quand l'invite de Claude revient"),
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

/// Whether `path` is a file Kova wrote for a pasted image
/// (`appkit_input::pasted_image_path`): `/tmp/kova-paste-<ms>.png`.
pub fn is_pasted_image(path: &str) -> bool {
    path.strip_prefix("/tmp/kova-paste-")
        .and_then(|rest| rest.strip_suffix(".png"))
        .is_some_and(|ms| !ms.is_empty() && ms.bytes().all(|b| b.is_ascii_digit()))
}

/// The message with the images pasted in the box: their paths first, as a
/// paste in the terminal hands them to Claude Code, then the text. Only
/// files Kova wrote for a paste go along.
pub fn with_images(text: &str, images: &[String]) -> String {
    let mut parts: Vec<&str> = images.iter().map(String::as_str).filter(|p| is_pasted_image(p)).collect();
    if !text.trim().is_empty() {
        parts.push(text);
    }
    parts.join(" ")
}

/// The text to paste: as is inside a bracketed paste; on one line when the
/// app did not ask for one, since each newline would be an Enter.
pub fn typed(text: &str, bracketed: bool) -> String {
    if bracketed { text.to_string() } else { text.replace('\n', " ") }
}

/// After an Enter, the next queued message waits this long at least: the
/// turn it started has to show (hook, spinner) before the gate reads idle.
pub const SETTLE_MS: u64 = 1_500;

/// A message waiting for its turn, editable and cancellable until it leaves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub id: u64,
    pub text: String,
}

/// The box's messages: the queue, in order, and the one typed and waiting
/// for its Enter. Times are milliseconds on any monotonic clock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outbox {
    queue: Vec<Pending>,
    next_id: u64,
    enter_due: Option<u64>,
    /// When the last Enter went: the next message waits `SETTLE_MS`.
    entered_at: Option<u64>,
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
}

impl Outbox {
    pub fn queue(&self) -> &[Pending] {
        &self.queue
    }

    /// A typed message waits for its Enter: the tick polls every frame.
    pub fn enter_pending(&self) -> bool {
        self.enter_due.is_some()
    }

    fn busy(&self, now: u64) -> bool {
        self.enter_due.is_some() || self.entered_at.is_some_and(|t| now < t + SETTLE_MS)
    }

    /// The user pressed Enter in the box with `text` (already sanitized). It
    /// goes now only when nothing waits before it: the order is kept.
    pub fn submit(&mut self, text: String, verdict: Verdict, now: u64) -> Step {
        if self.queue.is_empty() && !self.busy(now) && verdict == Verdict::Send {
            self.enter_due = Some(now + ENTER_DELAY_MS);
            return Step::Type(text);
        }
        self.next_id += 1;
        self.queue.push(Pending { id: self.next_id, text });
        Step::Queued(match verdict {
            Verdict::Wait(reason) => reason,
            Verdict::Send => "part après le message précédent",
        })
    }

    /// On the tick: the Enter of a typed message once its delay ran out, or
    /// the first queued message once the gate opens.
    pub fn poll(&mut self, verdict: Verdict, now: u64) -> Step {
        if let Some(due) = self.enter_due {
            if now < due {
                return Step::Nothing;
            }
            self.enter_due = None;
            self.entered_at = Some(now);
            return if now - due > ENTER_STALE_MS { Step::Stale } else { Step::Enter };
        }
        if self.queue.is_empty() || self.busy(now) || verdict != Verdict::Send {
            return Step::Nothing;
        }
        self.enter_due = Some(now + ENTER_DELAY_MS);
        Step::Type(self.queue.remove(0).text)
    }

    /// A queued message rewritten; false once it left (or never was).
    pub fn edit(&mut self, id: u64, text: String) -> bool {
        match self.queue.iter_mut().find(|p| p.id == id) {
            Some(p) => {
                p.text = text;
                true
            }
            None => false,
        }
    }

    /// A queued message taken back; `None` once it left.
    pub fn cancel(&mut self, id: u64) -> Option<String> {
        let i = self.queue.iter().position(|p| p.id == id)?;
        Some(self.queue.remove(i).text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_images_go_before_the_text_as_their_paths() {
        let images = vec!["/tmp/kova-paste-1793963274831.png".to_string(), "/etc/passwd".to_string(), "/tmp/kova-paste-x.png".to_string()];
        assert_eq!(with_images("look at this", &images), "/tmp/kova-paste-1793963274831.png look at this", "only a file Kova wrote");
        assert_eq!(with_images("  ", &images[..1]), "/tmp/kova-paste-1793963274831.png", "an image alone is a message");
        assert_eq!(with_images("hi", &[]), "hi");
        assert!(!is_pasted_image("/tmp/kova-paste-1.png\n"));
        assert_eq!(sanitize(&with_images("x", &images[..1])).as_deref(), Ok("/tmp/kova-paste-1793963274831.png x"));
    }

    fn idle() -> GateInput {
        GateInput { has_session: true, status: Banner::Done { at: None }, composer: Composer::Empty }
    }

    use crate::chat::banner::Banner;

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
    fn the_gate_sends_only_when_the_turn_is_over() {
        assert_eq!(gate(idle()), Verdict::Send);
        assert!(matches!(gate(GateInput { status: Banner::Working { since: None }, ..idle() }), Verdict::Wait(_)));
        assert!(matches!(gate(GateInput { status: Banner::Waiting, ..idle() }), Verdict::Wait(_)));
        assert!(matches!(gate(GateInput { composer: Composer::Unreadable, ..idle() }), Verdict::Wait(_)));
        assert!(matches!(gate(GateInput { has_session: false, ..idle() }), Verdict::Wait(_)), "kept until Claude is back");
        // A draft in the terminal never bounces the message: it goes with it.
        assert_eq!(gate(GateInput { composer: Composer::Typed, ..idle() }), Verdict::Send);
        assert!(matches!(gate(GateInput { status: Banner::Waiting, composer: Composer::Typed, ..idle() }), Verdict::Wait(_)));
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
        assert_eq!(out.submit("again".into(), Verdict::Send, 1100), Step::Queued("part après le message précédent"), "queued, never refused");
        assert_eq!(out.poll(Verdict::Wait("busy"), 1000 + ENTER_DELAY_MS), Step::Enter, "the Enter of a typed text always follows");
        assert_eq!(out.queue().len(), 1);
    }

    #[test]
    fn a_late_enter_is_dropped() {
        let mut out = Outbox::default();
        out.submit("hi".into(), Verdict::Send, 0);
        assert_eq!(out.poll(Verdict::Send, ENTER_DELAY_MS + ENTER_STALE_MS + 1), Step::Stale);
        assert_eq!(out.poll(Verdict::Send, 60_000), Step::Nothing);
    }

    #[test]
    fn queued_messages_leave_in_order_one_turn_each() {
        let mut out = Outbox::default();
        assert_eq!(out.submit("one".into(), Verdict::Wait("busy"), 0), Step::Queued("busy"));
        assert!(matches!(out.submit("two".into(), Verdict::Wait("busy"), 1), Step::Queued(_)));
        // The gate opens while one waits: a new message still goes behind it.
        assert!(matches!(out.submit("three".into(), Verdict::Send, 2), Step::Queued(_)));
        assert_eq!(out.queue().iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), ["one", "two", "three"]);
        assert_eq!(out.poll(Verdict::Wait("busy"), 10), Step::Nothing);
        assert_eq!(out.poll(Verdict::Send, 20), Step::Type("one".into()));
        assert_eq!(out.poll(Verdict::Send, 20 + ENTER_DELAY_MS), Step::Enter);
        // The turn it started has not shown yet: the next one waits.
        let entered = 20 + ENTER_DELAY_MS;
        assert_eq!(out.poll(Verdict::Send, entered + SETTLE_MS - 1), Step::Nothing);
        assert_eq!(out.poll(Verdict::Wait("busy"), entered + SETTLE_MS), Step::Nothing);
        assert_eq!(out.poll(Verdict::Send, entered + 9_000), Step::Type("two".into()));
    }

    #[test]
    fn a_queued_message_can_be_edited_or_cancelled_until_it_leaves() {
        let mut out = Outbox::default();
        out.submit("one".into(), Verdict::Wait("busy"), 0);
        out.submit("two".into(), Verdict::Wait("busy"), 0);
        let (one, two) = (out.queue()[0].id, out.queue()[1].id);
        assert!(out.edit(two, "two, better".into()));
        assert_eq!(out.cancel(one).as_deref(), Some("one"));
        assert_eq!(out.cancel(one), None, "already gone");
        assert_eq!(out.poll(Verdict::Send, 5), Step::Type("two, better".into()));
        assert!(!out.edit(two, "too late".into()), "it left");
        assert_eq!(out.cancel(two), None);
        assert!(out.queue().is_empty());
    }
}
