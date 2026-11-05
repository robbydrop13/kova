//! Chat mode: a pane's Claude conversation shown as a chat instead of its
//! terminal grid, with an input box and the pane's pending validations as
//! cards (`docs/chat-mode.md`).
//!
//! Everything here is pure and tested: reading the transcript as it grows
//! (`tail`), turning its lines into messages and tool calls
//! (`conversation`), the state banner (`banner`), what the input box may
//! type and when (`input`), the cards (`cards`) and the page (`page`). The
//! AppKit side, a web view laid over the pane, is `window::chat_view`.

pub mod banner;
pub mod cards;
pub mod conversation;
pub mod input;
pub mod page;
pub mod tail;

use std::path::{Path, PathBuf};

/// The directory name Claude Code gives a working directory under
/// `projects/`: everything but `[A-Za-z0-9._]` turned into `-` (the same
/// rule as `prompt_preview::transcript_path`).
pub fn project_slug(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c } else { '-' }).collect()
}

/// Where the transcript of Claude session `session_id` is: under its config
/// dir (`CLAUDE_CONFIG_DIR`, else `~/.claude`), in the folder of `cwd`, else
/// in whichever project folder holds it (a session started in another
/// directory than the pane's shell). The cwd's path when the file does not
/// exist yet: Claude Code writes it on the first message. `None` for an id
/// that could climb out of the folder.
pub fn transcript_path(home: &Path, config_dir: Option<&str>, cwd: Option<&str>, session_id: &str) -> Option<PathBuf> {
    if !crate::claude_session::is_safe_session_id(session_id) {
        return None;
    }
    let root = config_dir.map(PathBuf::from).unwrap_or_else(|| home.join(".claude"));
    let projects = root.join("projects");
    let file = format!("{session_id}.jsonl");
    let expected = cwd.map(|cwd| projects.join(project_slug(cwd)).join(&file));
    if expected.as_ref().is_some_and(|p| p.is_file()) {
        return expected;
    }
    let found = std::fs::read_dir(&projects)
        .ok()
        .and_then(|dirs| dirs.flatten().map(|d| d.path().join(&file)).find(|p| p.is_file()));
    found.or(expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transcript_is_found_where_claude_code_writes_it() {
        let home = std::env::temp_dir().join(format!("kova-chat-home-{}", crate::ask::new_question_id()));
        let elsewhere = home.join(".claude/projects/-other-dir");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let expected = home.join(".claude/projects/-Users-x-my-app/abc.jsonl");
        assert_eq!(transcript_path(&home, None, Some("/Users/x/my app"), "abc"), Some(expected.clone()), "not written yet");
        std::fs::write(elsewhere.join("abc.jsonl"), "").unwrap();
        assert_eq!(transcript_path(&home, None, Some("/Users/x/my app"), "abc"), Some(elsewhere.join("abc.jsonl")));
        std::fs::create_dir_all(expected.parent().unwrap()).unwrap();
        std::fs::write(&expected, "").unwrap();
        assert_eq!(transcript_path(&home, None, Some("/Users/x/my app"), "abc"), Some(expected));
        assert_eq!(transcript_path(&home, Some("/cfg"), None, "abc"), None, "nothing in that config dir, no cwd");
        assert_eq!(transcript_path(&home, None, Some("/x"), "../../etc/passwd"), None);
    }
}
