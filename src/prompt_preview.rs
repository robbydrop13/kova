//! What a Claude pane is asking, read off its screen: the permission prompt
//! grammar KovaLink's daemon parses (`daemon/src/prompt/parser.ts`), ported
//! as is, plus the turn-end summary read from the transcript tail. Pure: the
//! probe that feeds it lives in `Pane::probe_prompt`, the tile that shows it
//! in the sidebar. See `docs/sidebar-spec.md`, section 6.
//!
//! The grammar is observed, not invented (fixtures under
//! `tests/fixtures/prompt/`, captured on Claude Code 2.1.268):
//!
//! ```text
//! ─────────────────────────────────────    frame rule, full width
//!  Bash command                            header: the nature of the action
//!  Tip: auto mode handles these prompts    tip line, sometimes there
//!
//!    echo bonjour > hello.txt              detail lines
//!    Write "bonjour" to hello.txt
//!  Do you want to proceed?                 the question, ends with `?`
//!  ❯ 1. Yes                                options numbered from 1
//!    2. Yes, and always allow ...
//!    3. No
//!
//!  Esc to cancel · Tab to amend            footer, LAST non-empty line
//! ```
//!
//! The options are kept as `(number, label)` pairs, and the labels alone say
//! what each button of the Asks card does (`ParsedPrompt::choices`): the
//! first `Yes` that is not an `always` approves once, the one that always
//! allows is the card's `Always`, the `No` rejects. A screen whose option
//! block strays from the numbering (a prompt scrolled so option 1 is off
//! screen) is still a permission prompt, with no options: its card offers
//! only `Open`, and nothing may be typed into it.
//!
//! A `fingerprint` of the header, the question, the detail and the option
//! labels lets a click answer only the screen it was shown for: the card
//! holds the hash, the click reads the pane again and compares.
//!
//! Any other deviation yields `None`: a stale prompt higher up the screen does
//! not parse either, because the footer has to be the last non-empty line.

use std::path::{Path, PathBuf};

/// What the sidebar shows under a pane's title once its Claude stopped
/// working. Runtime state only, never saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptPreview {
    /// A permission prompt is on screen: the pane waits for an answer.
    Permission {
        header: String,
        question: String,
        detail: Option<String>,
        /// Epoch seconds when the prompt was detected.
        since: u64,
        /// The pane was looked at since the prompt appeared.
        seen: bool,
    },
    /// The turn ended with an answer; `seen` once the pane was looked at.
    TurnEnd { summary: String, seen: bool },
    /// A turn died on a provider error. `since` is when it was detected
    /// (epoch seconds), `seen` once the pane was looked at.
    ApiError { kind: String, text: String, since: u64, seen: bool },
}

/// A turn that ended on a provider error, as the sidebar tile says it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ApiError {
    /// The word the tile shows, from the transcript's `error` value.
    pub kind: String,
    /// What the provider said, one line, at most 120 chars.
    pub text: String,
}

/// The parts of a permission prompt the tile and the Asks card show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedPrompt {
    /// `Bash command`, `Create file`, ...
    pub header: String,
    /// `Do you want to proceed?`
    pub question: String,
    /// First detail line under the header (the command, the file name).
    pub detail: Option<String>,
    /// The numbered options, in order: `(1, "Yes")`, ... Empty when the block
    /// on screen does not number from 1 (a prompt scrolled partway): its card
    /// then offers only `Open`.
    pub options: Vec<(u32, String)>,
}

/// Which button of a permission card a click is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionChoice {
    /// The first `Yes` that is not an `always`.
    Approve,
    /// The option that always allows.
    Always,
    /// The `No`.
    Reject,
}

/// The digit each button of a permission card types, read off the labels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PermissionChoices {
    pub approve: Option<u32>,
    pub always: Option<u32>,
    pub reject: Option<u32>,
}

impl PermissionChoices {
    pub fn digit(self, choice: PermissionChoice) -> Option<u32> {
        match choice {
            PermissionChoice::Approve => self.approve,
            PermissionChoice::Always => self.always,
            PermissionChoice::Reject => self.reject,
        }
    }
}

impl ParsedPrompt {
    /// The option each card button sends, by label: the first `Yes` that is
    /// not an `always` approves, the one containing `always` or `don't ask
    /// again` is `Always`, the one starting with `No` rejects. A wording that
    /// matches none of them leaves the matching buttons out, and the card
    /// only offers `Open`.
    pub fn choices(&self) -> PermissionChoices {
        let mut out = PermissionChoices::default();
        for (number, label) in &self.options {
            let lower = label.to_ascii_lowercase();
            let always = lower.contains("always") || lower.contains("don't ask again");
            if out.always.is_none() && always {
                out.always = Some(*number);
            }
            if out.approve.is_none() && !always && label.starts_with("Yes") {
                out.approve = Some(*number);
            }
            if out.reject.is_none() && label.starts_with("No") {
                out.reject = Some(*number);
            }
        }
        out
    }

    /// A stable hash of the header, the question, the detail and the option
    /// labels, trimmed (FNV-1a): two reads of the same screen give the same
    /// number, a screen with another command or another prompt another one.
    pub fn fingerprint(&self) -> u64 {
        let mut h = Fingerprint::new();
        h.text(&self.header);
        h.text(&self.question);
        h.text(self.detail.as_deref().unwrap_or(""));
        for (number, label) in &self.options {
            h.number(*number);
            h.text(label);
        }
        h.0
    }
}

/// FNV-1a, 64 bit: fixed constants, so the fingerprint means the same thing
/// across runs and builds.
struct Fingerprint(u64);

impl Fingerprint {
    fn new() -> Self {
        Fingerprint(0xcbf2_9ce4_8422_2325)
    }

    fn byte(&mut self, b: u8) {
        self.0 ^= b as u64;
        self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
    }

    /// A trimmed field, closed by a unit separator so `a|b` and `ab` differ.
    fn text(&mut self, text: &str) {
        for b in text.trim().as_bytes() {
            self.byte(*b);
        }
        self.byte(0x1f);
    }

    fn number(&mut self, n: u32) {
        for b in n.to_le_bytes() {
            self.byte(b);
        }
    }
}

/// The digit a click may type: the one `choice` names, and only while `fresh`
/// is the same prompt as `old`. `None` means the screen moved on (another
/// command, another question): nothing may be typed into it.
pub fn permission_answer(
    old: &ParsedPrompt,
    fresh: Option<&ParsedPrompt>,
    choice: PermissionChoice,
) -> Option<u32> {
    let fresh = fresh?;
    if fresh.fingerprint() != old.fingerprint() {
        return None;
    }
    fresh.choices().digit(choice)
}

/// Sanity bound: past this it is not a permission prompt.
const MAX_OPTIONS: usize = 20;
/// Longest summary kept from a transcript, in chars.
const SUMMARY_MAX: usize = 120;
/// Bytes read from the end of a transcript for the turn-end summary.
const TRANSCRIPT_TAIL_BYTES: u64 = 64 * 1024;

/// An option line: optional `❯` / `>` marker, number, dot, label.
fn parse_option(line: &str) -> Option<(u32, String, bool)> {
    let s = line.trim_start();
    let (marked, s) = match s.strip_prefix('\u{276f}').or_else(|| s.strip_prefix('>')) {
        Some(rest) => (true, rest.trim_start()),
        None => (false, s),
    };
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() > 2 {
        return None;
    }
    let rest = &s[digits.len()..];
    let rest = rest.strip_prefix('.')?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let label = rest.trim();
    if label.is_empty() {
        return None;
    }
    Some((digits.parse().ok()?, label.to_string(), marked))
}

/// A line of the option block, even one `parse_option` refuses (`1.Yes`, the
/// label glued to the dot): the block's extent is what puts the question
/// above it, whatever the numbering.
fn is_option_line(line: &str) -> bool {
    let s = line.trim_start();
    let s = match s.strip_prefix('\u{276f}').or_else(|| s.strip_prefix('>')) {
        Some(rest) => rest.trim_start(),
        None => s,
    };
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0 && s[digits..].starts_with('.')
}

/// The full-width frame rule: a run of at least ten `─`.
fn is_frame(line: &str) -> bool {
    let t = line.trim();
    t.chars().count() >= 10 && t.chars().all(|c| c == '\u{2500}')
}

/// Any decorative rule, the frame or the inner `╌` separators.
fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.chars().count() >= 10 && t.chars().all(|c| matches!(c, '\u{2500}' | '\u{254c}' | '\u{2550}' | '\u{2504}' | '\u{2508}' | '\u{2501}'))
}

fn is_footer(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("Esc to cancel") && !t["Esc to cancel".len()..].starts_with(|c: char| c.is_alphanumeric())
}

fn is_tip(line: &str) -> bool {
    line.trim_start().starts_with("Tip: ")
}

/// Parse the visible text of a pane. `None` when the screen is not a
/// permission prompt at all; a prompt whose option block strays from the
/// numbering keeps its header and question, with no options.
pub fn parse_permission_prompt(visible: &str) -> Option<ParsedPrompt> {
    if visible.is_empty() {
        return None;
    }
    let lines: Vec<&str> = visible.split('\n').map(str::trim_end).collect();

    // 1. The footer is the LAST non-empty line of the screen.
    let footer_at = lines.iter().rposition(|l| !l.trim().is_empty())?;
    if !is_footer(lines[footer_at]) {
        return None;
    }

    // 2. The option block: contiguous lines right above the footer (blank
    //    lines tolerated in between). Its extent is read whether or not the
    //    numbers hold, so the question is always found above it; the options
    //    themselves are kept only when they number 1, 2, 3...
    let mut i = footer_at;
    while i > 0 && lines[i - 1].trim().is_empty() {
        i -= 1;
    }
    let mut collected = Vec::new();
    let mut well_formed = true;
    while i > 0 && is_option_line(lines[i - 1]) {
        match parse_option(lines[i - 1]) {
            Some(opt) => collected.push(opt),
            None => well_formed = false,
        }
        i -= 1;
    }
    collected.reverse();
    let numbered = well_formed
        && collected.len() >= 2
        && collected.len() <= MAX_OPTIONS
        && collected.iter().enumerate().all(|(k, (n, _, _))| *n == k as u32 + 1)
        && collected.iter().filter(|(_, _, marked)| *marked).count() <= 1;
    let options: Vec<(u32, String)> = if numbered {
        collected.into_iter().map(|(number, label, _)| (number, label)).collect()
    } else {
        Vec::new()
    };
    let first_option_at = i;

    // 3. The question: first non-empty line above the option block, ending
    //    with `?`.
    let mut q = first_option_at;
    while q > 0 && lines[q - 1].trim().is_empty() {
        q -= 1;
    }
    if q == 0 {
        return None;
    }
    q -= 1;
    let question = lines[q].trim();
    if !question.ends_with('?') || is_option_line(question) || is_rule(question) {
        return None;
    }

    // 4. The frame: from the last full-width rule above the question down to
    //    it. The header is the first non-empty line of the frame, mandatory.
    let top = lines[..q].iter().rposition(|l| is_frame(l))?;
    let body: Vec<&str> = lines[top + 1..q]
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !is_rule(l) && !is_tip(l))
        .collect();
    if body.is_empty() {
        return None;
    }
    if body.iter().any(|l| is_option_line(l) || is_footer(l)) {
        return None;
    }

    Some(ParsedPrompt {
        header: body[0].to_string(),
        question: question.to_string(),
        detail: body.get(1).map(|s| s.to_string()),
        options,
    })
}

// ---------------------------------------------------------------
// Turn-end summary from the transcript
// ---------------------------------------------------------------

/// `~/.claude/projects/<slug>/<session-id>.jsonl`, the slug being the cwd
/// with everything but `[A-Za-z0-9._]` turned into `-` (the daemon's
/// `projectSlug`).
pub fn transcript_path(home: &str, cwd: &str, session_id: &str) -> PathBuf {
    let slug: String = cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c } else { '-' })
        .collect();
    Path::new(home).join(".claude").join("projects").join(slug).join(format!("{session_id}.jsonl"))
}

/// The last 64 KB of a transcript, from a line boundary. `None` when the
/// file cannot be read.
pub fn read_transcript_tail(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TRANSCRIPT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if start == 0 {
        return Some(text);
    }
    // A cut in the middle of a line: drop the partial first line.
    Some(text.find('\n').map_or(String::new(), |i| text[i + 1..].to_string()))
}

/// The last thing the assistant said in a transcript tail: the last
/// non-empty `text` block of the last `assistant` record that has one, cut
/// to its first line, markdown marks stripped, tail-truncated with `…`.
pub fn turn_end_summary(transcript_tail: &str) -> Option<String> {
    let mut summary: Option<String> = None;
    for line in transcript_tail.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if record.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(blocks) = record.pointer("/message/content").and_then(|c| c.as_array()) else { continue };
        for block in blocks {
            if block.get("type").and_then(|t| t.as_str()) != Some("text") {
                continue;
            }
            if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                if !text.trim().is_empty() {
                    summary = Some(text.to_string());
                }
            }
        }
    }
    summary.map(|s| summary_line(&s)).filter(|s| !s.is_empty())
}

/// The provider error a transcript tail ends on: `Some` when the LAST `user`
/// or `assistant` record is an assistant one flagged `isApiErrorMessage`.
/// A later user line (the retry) or assistant line (a fresh answer) means the
/// pane moved on, so nothing is reported. Every other record type (summary,
/// system, attachment...) is ignored, including one written after the error.
///
/// Observed `error` values and the word the tile shows for each: `rate_limit`
/// is "Limit reached", `server_error` "API error", `invalid_request` "Request
/// failed", `authentication_failed` "Logged out", anything else "Error".
pub fn api_error(transcript_tail: &str) -> Option<ApiError> {
    for line in transcript_tail.lines().rev() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        match record.get("type").and_then(|t| t.as_str()) {
            Some("assistant") => {
                if record.get("isApiErrorMessage").and_then(|f| f.as_bool()) != Some(true) {
                    return None;
                }
                let text = record
                    .pointer("/message/content")
                    .and_then(|c| c.as_array())
                    .and_then(|blocks| blocks.iter().find_map(|b| b.get("text").and_then(|t| t.as_str())))
                    .map(summary_line)
                    .unwrap_or_default();
                let error = record.get("error").and_then(|e| e.as_str()).unwrap_or("");
                return Some(ApiError { kind: error_kind(error).to_string(), text });
            }
            Some("user") => return None,
            _ => {}
        }
    }
    None
}

/// The word the tile shows for a transcript `error` value.
fn error_kind(error: &str) -> &'static str {
    match error {
        "rate_limit" => "Limit reached",
        "server_error" => "API error",
        "invalid_request" => "Request failed",
        "authentication_failed" => "Logged out",
        _ => "Error",
    }
}

/// One line out of a markdown answer: the first non-empty line, leading
/// heading / list marks and every emphasis mark stripped, cut at
/// `SUMMARY_MAX` chars with an ellipsis.
pub fn summary_line(text: &str) -> String {
    let line = text
        .lines()
        .map(|l| l.trim().trim_start_matches(|c| matches!(c, '#' | '*' | '-' | '>' | ' ')))
        .find(|l| !l.is_empty() && !l.starts_with('<'))
        .unwrap_or("");
    let cleaned: String = line.chars().filter(|c| !matches!(c, '*' | '`')).collect();
    let cleaned = cleaned.trim();
    let count = cleaned.chars().count();
    if count <= SUMMARY_MAX {
        return cleaned.to_string();
    }
    let mut out: String = cleaned.chars().take(SUMMARY_MAX - 1).collect();
    out.push('\u{2026}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASH: &str = include_str!("../tests/fixtures/prompt/prompt-bash.txt");
    const BASH_1: &str = include_str!("../tests/fixtures/prompt/prompt-bash-consecutive-1.txt");
    const BASH_2: &str = include_str!("../tests/fixtures/prompt/prompt-bash-consecutive-2.txt");
    const BASH_THREE: &str = include_str!("../tests/fixtures/prompt/prompt-bash-three.txt");
    const BASH_TWO: &str = include_str!("../tests/fixtures/prompt/prompt-bash-two-options.txt");
    const EDIT_FILE: &str = include_str!("../tests/fixtures/prompt/prompt-edit-file.txt");
    const SCROLLED: &str = include_str!("../tests/fixtures/prompt/prompt-scrolled.txt");
    const WRITE: &str = include_str!("../tests/fixtures/prompt/prompt-write.txt");
    const IDLE: &str = include_str!("../tests/fixtures/prompt/screen-idle.txt");
    const TRUST: &str = include_str!("../tests/fixtures/prompt/screen-trust-dialog.txt");
    const TRANSCRIPT: &str = include_str!("../tests/fixtures/prompt/transcript-echo.jsonl");
    const ERROR_LAST: &str = include_str!("../tests/fixtures/transcripts/api_error_last.jsonl");
    const ERROR_RETRY: &str = include_str!("../tests/fixtures/transcripts/api_error_then_retry.jsonl");
    const ERROR_ANSWER: &str = include_str!("../tests/fixtures/transcripts/api_error_then_answer.jsonl");
    const NO_ERROR: &str = include_str!("../tests/fixtures/transcripts/no_error.jsonl");

    #[test]
    fn a_bash_prompt_parses_to_its_header_question_and_command() {
        let p = parse_permission_prompt(BASH).expect("bash prompt");
        assert_eq!(p.header, "Bash command");
        assert_eq!(p.question, "Do you want to proceed?");
        assert_eq!(p.detail.as_deref(), Some("echo bonjour > hello.txt"));
    }

    #[test]
    fn two_consecutive_prompts_differ_only_by_their_detail() {
        let a = parse_permission_prompt(BASH_1).expect("first");
        let b = parse_permission_prompt(BASH_2).expect("second");
        assert_eq!(a.question, b.question);
        assert_eq!(a.header, b.header);
        assert_eq!(a.detail.as_deref(), Some("echo un > un.txt"));
        assert_eq!(b.detail.as_deref(), Some("echo deux > deux.txt"));
    }

    #[test]
    fn a_write_prompt_skips_the_inner_rules_and_keeps_the_file_name() {
        let p = parse_permission_prompt(WRITE).expect("write prompt");
        assert_eq!(p.header, "Create file");
        assert_eq!(p.question, "Do you want to create notes.md?");
        assert_eq!(p.detail.as_deref(), Some("notes.md"));
    }

    #[test]
    fn an_idle_screen_and_the_trust_dialog_are_not_prompts() {
        assert_eq!(parse_permission_prompt(IDLE), None);
        // Options without numbers, footer "Enter to confirm · Esc to cancel".
        assert_eq!(parse_permission_prompt(TRUST), None);
        assert_eq!(parse_permission_prompt(""), None);
    }

    #[test]
    fn a_prompt_left_above_newer_output_does_not_parse() {
        let stale = format!("{BASH}\n⏺ Done.\n");
        assert_eq!(parse_permission_prompt(&stale), None);
    }

    #[test]
    fn options_that_do_not_number_from_one_leave_the_prompt_without_options() {
        // A prompt scrolled so option 1 sits above the visible area: still a
        // permission prompt, with nothing to type.
        let scrolled = parse_permission_prompt(SCROLLED).expect("still a prompt");
        assert_eq!(scrolled.question, "Do you want to proceed?");
        assert!(scrolled.options.is_empty());
        assert_eq!(scrolled.choices(), PermissionChoices::default());

        let broken = BASH.replace("   4. No", "   5. No");
        let p = parse_permission_prompt(&broken).expect("the frame and question hold");
        assert!(p.options.is_empty(), "{:?}", p.options);

        let one_option = "────────────────\n Bash command\n cmd\n Proceed?\n ❯ 1. Yes\n\n Esc to cancel\n";
        assert!(parse_permission_prompt(one_option).expect("still a prompt").options.is_empty());
        // No question, no prompt.
        assert_eq!(parse_permission_prompt("Esc to cancel\n"), None);
    }

    #[test]
    fn the_options_are_kept_in_order_with_their_numbers() {
        let bash = parse_permission_prompt(BASH_THREE).expect("bash prompt");
        let labels: Vec<&str> = bash.options.iter().map(|(_, label)| label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Yes",
                "Yes, and don't ask again for cargo test in /Users/test/project",
                "No, and tell Claude what to do differently",
            ]
        );
        assert_eq!(bash.options.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [1, 2, 3]);

        let edit = parse_permission_prompt(EDIT_FILE).expect("edit prompt");
        assert_eq!(edit.header, "Edit file");
        assert_eq!(edit.detail.as_deref(), Some("src/main.rs"));
        assert_eq!(edit.question, "Do you want to make this edit to main.rs?");
        assert_eq!(edit.options.len(), 3);

        let two = parse_permission_prompt(BASH_TWO).expect("two options");
        assert_eq!(two.options, [(1, "Yes".to_string()), (2, "No".to_string())]);
    }

    #[test]
    fn the_labels_decide_what_each_card_button_types() {
        let bash = parse_permission_prompt(BASH_THREE).expect("bash");
        assert_eq!(
            bash.choices(),
            PermissionChoices { approve: Some(1), always: Some(2), reject: Some(3) }
        );
        // The four-option capture: the auto-mode option starts with `Yes` but
        // is neither an always nor the first one, so approve stays option 1.
        let four = parse_permission_prompt(BASH).expect("four options");
        assert_eq!(four.choices(), PermissionChoices { approve: Some(1), always: Some(2), reject: Some(4) });
        // The write capture offers no always.
        let write = parse_permission_prompt(WRITE).expect("write");
        assert_eq!(write.choices(), PermissionChoices { approve: Some(1), always: None, reject: Some(3) });
        // Two options, no always.
        let two = parse_permission_prompt(BASH_TWO).expect("two");
        assert_eq!(two.choices(), PermissionChoices { approve: Some(1), always: None, reject: Some(2) });

        // A wording Kova has not seen: no button names an option.
        let unseen = ParsedPrompt {
            header: "Bash command".into(),
            question: "Do you want to proceed?".into(),
            detail: None,
            options: vec![(1, "Sure".into()), (2, "Later".into())],
        };
        assert_eq!(unseen.choices(), PermissionChoices::default());
    }

    #[test]
    fn the_fingerprint_holds_the_screen_and_follows_the_command() {
        let a = parse_permission_prompt(BASH_1).expect("first");
        let b = parse_permission_prompt(BASH_2).expect("second");
        assert_eq!(a.fingerprint(), parse_permission_prompt(BASH_1).expect("again").fingerprint());
        assert_ne!(a.fingerprint(), b.fingerprint(), "another command is another prompt");

        let mut other_question = a.clone();
        other_question.question = "Do you still want to proceed?".into();
        assert_ne!(a.fingerprint(), other_question.fingerprint());
        let mut relabelled = a.clone();
        relabelled.options[1].1 = "Yes, and always allow everything".into();
        assert_ne!(a.fingerprint(), relabelled.fingerprint());
        let mut no_options = a.clone();
        no_options.options.clear();
        assert_ne!(a.fingerprint(), no_options.fingerprint());
    }

    #[test]
    fn a_click_types_only_while_the_screen_is_the_one_the_card_shows() {
        let old = parse_permission_prompt(BASH_1).expect("first");
        let same = parse_permission_prompt(BASH_1).expect("again");
        assert_eq!(permission_answer(&old, Some(&same), PermissionChoice::Approve), Some(1));
        assert_eq!(permission_answer(&old, Some(&same), PermissionChoice::Always), Some(2));
        assert_eq!(permission_answer(&old, Some(&same), PermissionChoice::Reject), Some(4));

        // Another command under the same question: nothing may be typed.
        let moved = parse_permission_prompt(BASH_2).expect("second");
        for choice in [PermissionChoice::Approve, PermissionChoice::Always, PermissionChoice::Reject] {
            assert_eq!(permission_answer(&old, Some(&moved), choice), None);
        }
        // The prompt is gone entirely.
        assert_eq!(permission_answer(&old, None, PermissionChoice::Approve), None);
        // A prompt with no options offers no digit, unchanged or not.
        let empty = parse_permission_prompt(SCROLLED).expect("scrolled");
        assert_eq!(permission_answer(&empty, Some(&empty), PermissionChoice::Approve), None);
    }

    #[test]
    fn option_lines_follow_the_marker_number_dot_label_shape() {
        assert_eq!(parse_option(" ❯ 1. Yes"), Some((1, "Yes".to_string(), true)));
        assert_eq!(parse_option("   12. No, and tell Claude what to do  "), Some((12, "No, and tell Claude what to do".to_string(), false)));
        assert_eq!(parse_option("   1.Yes"), None);
        assert_eq!(parse_option("   123. Yes"), None);
        assert_eq!(parse_option("   1. "), None);
        assert_eq!(parse_option("Do you want to proceed?"), None);
    }

    #[test]
    fn the_transcript_tail_yields_the_last_assistant_text_as_one_clean_line() {
        let s = turn_end_summary(TRANSCRIPT).expect("summary");
        assert!(s.starts_with("The file is renamed."), "{s}");
        assert!(!s.contains('*'));
        assert!(s.chars().count() <= 120);
        assert_eq!(turn_end_summary(""), None);
        assert_eq!(turn_end_summary("{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n"), None);
    }

    #[test]
    fn summary_line_strips_markdown_and_truncates() {
        assert_eq!(summary_line("# Pushed the landing page copy\n\nMore."), "Pushed the landing page copy");
        assert_eq!(summary_line("- `main.rs` **done**"), "main.rs done");
        assert_eq!(summary_line("\n\n<system-reminder>x</system-reminder>\nReal line"), "Real line");
        let long = "a".repeat(200);
        let s = summary_line(&long);
        assert_eq!(s.chars().count(), 120);
        assert!(s.ends_with('\u{2026}'));
    }

    #[test]
    fn a_turn_that_died_on_an_api_error_is_read_from_the_transcript_tail() {
        let e = api_error(ERROR_LAST).expect("rate limit");
        assert_eq!(e.kind, "Limit reached");
        assert_eq!(e.text, "You've hit your session limit · resets 5:20pm (Europe/Paris)");
        // A later turn, whether retried or answered, clears it.
        assert_eq!(api_error(ERROR_RETRY), None);
        assert_eq!(api_error(ERROR_ANSWER), None);
        assert_eq!(api_error(NO_ERROR), None);
        assert_eq!(api_error(""), None);
    }

    #[test]
    fn every_observed_error_value_gets_a_tile_word() {
        for (raw, kind) in [
            ("rate_limit", "Limit reached"),
            ("server_error", "API error"),
            ("invalid_request", "Request failed"),
            ("authentication_failed", "Logged out"),
            ("model_not_found", "Error"),
            ("unknown", "Error"),
        ] {
            let line = format!(
                r#"{{"type":"assistant","isApiErrorMessage":true,"error":"{raw}","message":{{"content":[{{"type":"text","text":"failed"}}]}}}}"#
            );
            assert_eq!(api_error(&line).unwrap().kind, kind);
        }
    }

    #[test]
    fn an_error_text_is_one_line_of_at_most_120_chars() {
        let long = "a".repeat(300);
        let line = format!(
            r#"{{"type":"assistant","isApiErrorMessage":true,"error":"unknown","message":{{"content":[{{"type":"text","text":"{long}\nsecond line"}}]}}}}"#
        );
        let e = api_error(&line).unwrap();
        assert_eq!(e.text.chars().count(), 120);
        assert!(e.text.ends_with('\u{2026}'));
        assert!(!e.text.contains('\n'));
    }

    #[test]
    fn transcript_path_slugs_the_cwd_like_the_daemon() {
        let p = transcript_path("/Users/alice", "/Users/alice/dev/link", "abc-123");
        assert_eq!(p, PathBuf::from("/Users/alice/.claude/projects/-Users-alice-dev-link/abc-123.jsonl"));
    }

    // The last user or assistant line decides: records of any other type,
    // written after the error, do not cancel it.
    #[test]
    fn only_the_last_user_or_assistant_record_decides() {
        let error = r#"{"type":"assistant","isApiErrorMessage":true,"error":"rate_limit","message":{"content":[{"type":"text","text":"limit"}]}}"#;
        assert!(api_error(&format!("{error}\n{{\"type\":\"summary\",\"summary\":\"later\"}}\n")).is_some());
        assert!(api_error(&format!("not json\n{error}\n")).is_some());
        assert_eq!(api_error(&format!("{error}\n{{\"type\":\"user\",\"message\":{{\"content\":\"retry\"}}}}\n")), None);
    }
}
