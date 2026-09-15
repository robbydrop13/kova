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
//! Any deviation yields `None`: a stale prompt higher up the screen does not
//! parse either, because the footer has to be the last non-empty line.

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
}

/// The parts of a permission prompt the tile shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedPrompt {
    /// `Bash command`, `Create file`, ...
    pub header: String,
    /// `Do you want to proceed?`
    pub question: String,
    /// First detail line under the header (the command, the file name).
    pub detail: Option<String>,
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

/// Parse the visible text of a pane. `None` as soon as the rendering strays
/// from the observed grammar: expected, not an error.
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
    //    lines tolerated in between), numbered consecutively from 1.
    let mut i = footer_at;
    while i > 0 && lines[i - 1].trim().is_empty() {
        i -= 1;
    }
    let mut collected = Vec::new();
    while i > 0 {
        match parse_option(lines[i - 1]) {
            Some(opt) => {
                collected.push(opt);
                i -= 1;
            }
            None => break,
        }
    }
    collected.reverse();
    if collected.len() < 2 || collected.len() > MAX_OPTIONS {
        return None;
    }
    if collected.iter().enumerate().any(|(k, (n, _, _))| *n != k as u32 + 1) {
        return None;
    }
    if collected.iter().filter(|(_, _, marked)| *marked).count() > 1 {
        return None;
    }
    let first_option_at = i;

    // 3. The question: first non-empty line above option 1, ending with `?`.
    let mut q = first_option_at;
    while q > 0 && lines[q - 1].trim().is_empty() {
        q -= 1;
    }
    if q == 0 {
        return None;
    }
    q -= 1;
    let question = lines[q].trim();
    if !question.ends_with('?') || parse_option(question).is_some() || is_rule(question) {
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
    if body.iter().any(|l| parse_option(l).is_some() || is_footer(l)) {
        return None;
    }

    Some(ParsedPrompt {
        header: body[0].to_string(),
        question: question.to_string(),
        detail: body.get(1).map(|s| s.to_string()),
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
    const WRITE: &str = include_str!("../tests/fixtures/prompt/prompt-write.txt");
    const IDLE: &str = include_str!("../tests/fixtures/prompt/screen-idle.txt");
    const TRUST: &str = include_str!("../tests/fixtures/prompt/screen-trust-dialog.txt");
    const TRANSCRIPT: &str = include_str!("../tests/fixtures/prompt/transcript-echo.jsonl");

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
    fn options_must_be_numbered_from_one_without_gaps() {
        let broken = BASH.replace("   4. No", "   5. No");
        assert_eq!(parse_permission_prompt(&broken), None);
        let one_option = "────────────────\n Bash command\n cmd\n Proceed?\n ❯ 1. Yes\n\n Esc to cancel\n";
        assert_eq!(parse_permission_prompt(one_option), None);
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
        assert!(s.starts_with("Les fichiers, oui : 0 trace."), "{s}");
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
    fn transcript_path_slugs_the_cwd_like_the_daemon() {
        let p = transcript_path("/Users/rob", "/Users/rob/AI directory/link", "abc-123");
        assert_eq!(p, PathBuf::from("/Users/rob/.claude/projects/-Users-rob-AI-directory-link/abc-123.jsonl"));
    }
}
