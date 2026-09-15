//! One face for the coding agents a pane can be running — Claude Code and
//! Codex — so everything downstream (pane title, Cmd+J, session restore, IPC)
//! treats them alike instead of knowing only about Claude.
//!
//! The per-agent detection stays in `claude_session` and `codex_session`: they
//! find a live session in completely different ways (a file per process on one
//! side, the open transcript of a process on the other). This module only picks
//! between them and says how to reopen what it found.

use serde::{Deserialize, Serialize};

/// Which agent a pane is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    /// Name used in the session file and over IPC. Stable: a restore reads it.
    pub fn as_str(&self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }
}

/// A conversation open in a pane, whichever agent holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSession {
    pub agent: Agent,
    /// Conversation id — what the agent's `resume` takes.
    pub id: String,
    /// Conversation name: Claude's explicit `/rename` or Codex's persisted name.
    pub name: Option<String>,
}

/// The agent session running under `shell_pid`, if any.
///
/// Claude is asked first: its lookup is a small directory read, where the Codex
/// one walks the process table. A pane runs one agent at a time, so the second
/// scan only happens for panes that are not Claude.
pub fn for_shell(shell_pid: u32) -> Option<AgentSession> {
    if let Some(s) = crate::claude_session::session_for_shell(shell_pid) {
        return Some(AgentSession { agent: Agent::Claude, id: s.id, name: s.name });
    }
    let s = crate::codex_session::for_shell(shell_pid)?;
    Some(AgentSession { agent: Agent::Codex, id: s.id, name: s.name })
}

/// The command line that reopens this conversation, or `None` when the id is
/// not one we are willing to type into a shell.
///
/// `last_command` is the line the pane last ran: on the Claude side it carries
/// the flags the session was started with, which the resume line reuses.
pub fn resume_command(agent: Agent, session_id: &str, last_command: Option<&str>) -> Option<String> {
    match agent {
        Agent::Claude => crate::claude_session::resume_command(last_command, session_id),
        Agent::Codex => crate::codex_session::resume_command(session_id),
    }
}

/// The conversation a command line hands back, and to which agent: `claude …
/// --resume <id>` (or `-r <id>`) and `codex resume <id>`, the lines
/// `resume_command` builds and a restored pane keeps as its last command.
/// `None` for anything else, a plain `claude` included: that starts a
/// conversation, it does not resume one. The id comes back unvalidated; a
/// caller that types it into a shell goes through `resume_command`.
pub fn resumed_session(command: &str) -> Option<(Agent, String)> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let first = tokens.first()?;
    let name = first.rsplit('/').next().unwrap_or(first);
    match name {
        "claude" => {
            let at = tokens.iter().position(|t| *t == "--resume" || *t == "-r")?;
            let id = tokens.get(at + 1).filter(|id| !id.starts_with('-'))?;
            Some((Agent::Claude, id.to_string()))
        }
        "codex" if tokens.get(1) == Some(&"resume") => tokens.get(2).map(|id| (Agent::Codex, id.to_string())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resume_line_names_its_agent_and_a_plain_launch_does_not() {
        let claude = |id: &str| Some((Agent::Claude, id.to_string()));
        assert_eq!(resumed_session("claude --resume abc-123"), claude("abc-123"));
        assert_eq!(resumed_session("claude --dangerously-skip-permissions -r abc"), claude("abc"));
        assert_eq!(resumed_session("/opt/bin/claude --resume abc"), claude("abc"));
        assert_eq!(resumed_session("codex resume 01a0"), Some((Agent::Codex, "01a0".to_string())));
        assert_eq!(resumed_session("claude --resume"), None);
        assert_eq!(resumed_session("claude --resume --verbose"), None);
        assert_eq!(resumed_session("claude"), None);
        assert_eq!(resumed_session("codex"), None);
        assert_eq!(resumed_session("npm run dev"), None);
        assert_eq!(resumed_session(""), None);
    }


    #[test]
    fn each_agent_resumes_with_its_own_command() {
        let id = "01a07651-015e-78a3-97f2-2eaf0f0cd663";
        assert_eq!(
            resume_command(Agent::Codex, id, None).as_deref(),
            Some("codex resume 01a07651-015e-78a3-97f2-2eaf0f0cd663")
        );
        let claude = resume_command(Agent::Claude, id, None).expect("a claude line");
        assert!(claude.starts_with("claude "), "got {}", claude);
        assert!(claude.contains(id));
    }

    #[test]
    fn an_unsafe_id_is_refused_for_both() {
        assert!(resume_command(Agent::Claude, "id\nrm -rf ~", None).is_none());
        assert!(resume_command(Agent::Codex, "id\nrm -rf ~", None).is_none());
    }

    #[test]
    fn the_agent_name_round_trips_through_the_session_file() {
        for agent in [Agent::Claude, Agent::Codex] {
            let json = serde_json::to_string(&agent).unwrap();
            assert_eq!(json, format!("\"{}\"", agent.as_str()));
            assert_eq!(serde_json::from_str::<Agent>(&json).unwrap(), agent);
        }
    }
}
