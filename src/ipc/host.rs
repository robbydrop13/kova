//! The pane commands of the socket, written once against the app that holds
//! the panes.
//!
//! The transport (`crate::ipc`) parses a request into an `IpcCommand` and hands
//! it to the main thread. There, `dispatch` runs the commands that only need
//! "the panes and tabs this app holds" against an `IpcHost`, and gives every
//! other command back to the app. Kova.app implements `IpcHost` over its
//! windows (`crate::app`); another host (a library host, one tab per document) implements the
//! same trait and gets the same commands, the same JSON and the same errors.
//!
//! What a command does to one pane (`send_keys_to`, `pane_text_entry`…) is
//! here too, so every host writes to and reads from a pane the same way.
//!
//! Everything in this module runs on the main thread, like the rest of the
//! command handling: the host's panes are not `Send`.

use super::{pane_content_response, pane_count_response, IpcCommand, IpcResponse, PaneFilter};
use crate::surface::{PaneId, Surface};
use crate::terminal::DumpMode;

/// What `IpcHost::pane_signal` does to a pane: the IPC stand-ins for BEL,
/// OSC 133;D and OSC 133;C, for hooks that must not write on the tty.
#[derive(Debug, Clone, Copy)]
pub enum PaneSignal {
    Bell,
    Completed,
    Started,
}

impl PaneSignal {
    /// Raise the flag on the pane's terminal, as the tty sequence would.
    pub fn apply(self, surface: &Surface) {
        let term = surface.terminal.read();
        match self {
            PaneSignal::Bell => term.ring_bell(),
            PaneSignal::Completed => term.mark_command_completed(),
            PaneSignal::Started => term.mark_command_started(),
        }
    }
}

/// Outcome of `IpcHost::close_tab` (and of `KovaView::ipc_close_tab` for one
/// window). Lets the caller distinguish "not in this window" (keep scanning)
/// from "last tab — refuse" (final answer).
pub enum IpcCloseTabResult {
    Closed,
    WouldTerminate,
    NotFound,
}

/// One `list-tabs` entry. The shape is the host-independent contract a remote client reads
/// (`tab_json`); a host fills it from whatever it calls a tab.
pub struct TabInfo {
    pub id: u32,
    /// Index of the window in the host's window list.
    pub window: usize,
    /// Index of the tab in its window.
    pub tab_index: usize,
    pub title: String,
    pub pane_count: usize,
    pub focused_pane_id: PaneId,
    /// The active tab of the key window.
    pub active: bool,
    pub has_bell: bool,
    pub has_completion: bool,
    pub has_running: bool,
}

/// The JSON of one `list-tabs` entry.
pub fn tab_json(tab: &TabInfo) -> serde_json::Value {
    serde_json::json!({
        "id": tab.id,
        "window": tab.window,
        "tab_index": tab.tab_index,
        "title": tab.title,
        "pane_count": tab.pane_count,
        "focused_pane_id": tab.focused_pane_id,
        "active": tab.active,
        "has_bell": tab.has_bell,
        "has_completion": tab.has_completion,
        "has_running": tab.has_running,
    })
}

/// What an app gives the socket so the pane commands can run against it.
///
/// Every method that names a pane or a tab answers "not found" (`false`,
/// `None`, `NotFound`) when the host holds no such id; the error strings are
/// written once, in this module.
pub trait IpcHost {
    /// `list-panes`: one entry per pane, in the host's display order. Kova.app
    /// builds them with `crate::window::pane_json`, the shape `subscribe` and
    /// the events share.
    fn collect_panes(&self, out: &mut Vec<serde_json::Value>);
    /// Every pane id, in `collect_panes` order (expands `panes: "all"`).
    fn collect_pane_ids(&self, out: &mut Vec<PaneId>);
    /// `send-keys`: write `text` to the pane with `send_keys_to`.
    fn send_keys(&self, pane_id: PaneId, text: &str) -> bool;
    /// What runs in the pane a `send-keys` names (`typing_target_of`): the
    /// journal describes it and the tag rule only holds for Claude Code.
    fn typing_target(&self, pane_id: PaneId) -> Option<crate::typed_keys::Target>;
    /// One `get-pane-content` entry, built with `pane_text_entry`.
    fn dump_pane_text(&self, pane_id: PaneId, mode: DumpMode, trim: bool) -> Option<serde_json::Value>;
    /// `(chars, bytes)` of that entry's text, from `measure_pane_text`.
    fn measure_pane_text(&self, pane_id: PaneId, mode: DumpMode, trim: bool) -> Option<(usize, usize)>;
    /// The pane's "finished" flag (`completion_flag`).
    fn check_completion(&self, pane_id: PaneId) -> Option<bool>;
    /// Bring the pane to the front: its tab, its window, the app.
    fn focus_pane(&self, pane_id: PaneId) -> bool;
    /// Close the pane. `Some(false)`: found, but the host refuses (its last pane).
    fn close_pane(&self, pane_id: PaneId) -> Option<bool>;
    /// Set or clear the pane's "waiting for the user" flag, and redraw.
    fn set_pane_status(&self, pane_id: PaneId, waiting: bool) -> bool;
    /// Apply `signal` to the pane (`PaneSignal::apply`), and redraw.
    fn pane_signal(&self, pane_id: PaneId, signal: PaneSignal) -> bool;
    /// Post a desktop notification whose click focuses `pane_id`.
    fn notify(&self, pane_id: Option<PaneId>, title: &str, message: &str, sound: bool) -> Result<(), String>;
    /// `list-tabs`: every tab, in the host's display order.
    fn collect_tabs(&self, out: &mut Vec<TabInfo>);
    /// Close the tab with this id.
    fn close_tab(&self, tab_id: u32) -> IpcCloseTabResult;
}

/// What `dispatch` decided to do with a host command.
pub enum Disposition {
    /// Reply to the client now with this response.
    Reply(IpcResponse),
    /// Defer the response — the main thread keeps polling until the wait
    /// resolves (or its deadline passes), at which point it sends back.
    Pending(DeferredWait),
}

/// A wait that the main thread has accepted but not yet resolved.
pub struct DeferredWait {
    pub pane_id: u32,
    pub deadline: std::time::Instant,
}

/// A `wait-for-completion` request the main thread is still polling.
pub struct PendingWait {
    pub pane_id: u32,
    pub response_tx: std::sync::mpsc::Sender<IpcResponse>,
    pub deadline: std::time::Instant,
}

/// Run `cmd` against `host` if it is a pane or `list-tabs`/`close-tab` command;
/// hand it back untouched otherwise, for the app's own dispatcher. Most
/// commands reply synchronously; only `wait-for-completion` may return
/// `Pending` so the main thread can defer the reply.
pub fn dispatch(host: &impl IpcHost, cmd: IpcCommand) -> Result<Disposition, IpcCommand> {
    let response = match cmd {
        IpcCommand::WaitForCompletion { pane_id, timeout_ms } => {
            return Ok(wait_for_completion(host, pane_id, timeout_ms));
        }
        IpcCommand::ListPanes => list_panes(host),
        IpcCommand::Doctor => IpcResponse::Ok { data: Some(crate::doctor::report()) },
        IpcCommand::ClosePaneById(pane_id) => close_pane(host, pane_id),
        IpcCommand::SendKeys { pane_id, text, caller } => send_keys(host, pane_id, &text, &caller),
        IpcCommand::FocusPane(pane_id) => focus_pane(host, pane_id),
        IpcCommand::GetPaneContent { panes, mode, trim_trailing_blank_lines } => {
            get_pane_content(host, panes, &mode, trim_trailing_blank_lines)
        }
        IpcCommand::CountPaneContent { panes, mode, trim_trailing_blank_lines } => {
            count_pane_content(host, panes, &mode, trim_trailing_blank_lines)
        }
        IpcCommand::ListTabs => list_tabs(host),
        IpcCommand::CloseTab(tab_id) => close_tab(host, tab_id),
        IpcCommand::SetPaneStatus { pane_id, waiting } => set_pane_status(host, pane_id, waiting),
        IpcCommand::MarkCompleted { pane_id } => pane_signal(host, pane_id, PaneSignal::Completed),
        IpcCommand::Bell { pane_id } => pane_signal(host, pane_id, PaneSignal::Bell),
        IpcCommand::MarkStarted { pane_id } => pane_signal(host, pane_id, PaneSignal::Started),
        IpcCommand::Notify { pane_id, title, message, sound } => {
            notify(host, pane_id, &title, &message, sound)
        }
        other => return Err(other),
    };
    Ok(Disposition::Reply(response))
}

fn not_found(pane_id: PaneId) -> IpcResponse {
    IpcResponse::Error { message: format!("pane {} not found", pane_id) }
}

/// Ok with no data when `found`, "pane N not found" otherwise.
fn found_or_not(found: bool, pane_id: PaneId) -> IpcResponse {
    if found {
        IpcResponse::Ok { data: None }
    } else {
        not_found(pane_id)
    }
}

/// `list-panes`: every pane of the host.
pub fn list_panes(host: &impl IpcHost) -> IpcResponse {
    let mut panes = Vec::new();
    host.collect_panes(&mut panes);
    IpcResponse::Ok {
        data: Some(serde_json::Value::Array(panes)),
    }
}

/// `close-pane`.
pub fn close_pane(host: &impl IpcHost, pane_id: PaneId) -> IpcResponse {
    match host.close_pane(pane_id) {
        Some(true) => IpcResponse::Ok { data: None },
        Some(false) => IpcResponse::Error { message: format!("pane {} is the last pane — cannot close", pane_id) },
        None => not_found(pane_id),
    }
}

/// `send-keys`, under the policy in force (`crate::typed_keys`).
pub fn send_keys(host: &impl IpcHost, pane_id: PaneId, text: &str, caller: &crate::typed_keys::Caller) -> IpcResponse {
    send_keys_under(host, pane_id, text, caller, &crate::typed_keys::policy())
}

/// `send-keys` under `policy`: every send to an existing pane is journaled,
/// and untagged text to a pane running Claude Code is typed, flagged or
/// refused as the mode says.
pub fn send_keys_under(
    host: &impl IpcHost,
    pane_id: PaneId,
    text: &str,
    caller: &crate::typed_keys::Caller,
    policy: &crate::typed_keys::Policy,
) -> IpcResponse {
    use crate::typed_keys::{self, Verdict};
    let Some(target) = host.typing_target(pane_id) else {
        return not_found(pane_id);
    };
    let typed = typed_keys::read(text);
    let verdict = typed_keys::verdict(policy.mode, typed.kind, target.claude);
    if let Some(journal) = &policy.journal {
        let line = typed_keys::entry(&typed_keys::now_utc(), pane_id, &target, text, &typed, policy.mode, verdict, caller);
        typed_keys::append(journal, &line);
    }
    if verdict != Verdict::Accepted {
        // Never the text: a pane line may be private.
        log::warn!(
            "IPC send-keys: untagged text ({} bytes) to pane {} running Claude Code, from pid {:?} ({}): {}",
            text.len(),
            pane_id,
            caller.pid,
            caller.name.as_deref().unwrap_or("unknown"),
            verdict.as_str()
        );
    }
    if verdict == Verdict::Refused {
        return IpcResponse::Error { message: typed_keys::REFUSAL.to_string() };
    }
    found_or_not(host.send_keys(pane_id, text), pane_id)
}

/// `focus-pane`, and the click on a notification.
pub fn focus_pane(host: &impl IpcHost, pane_id: PaneId) -> IpcResponse {
    found_or_not(host.focus_pane(pane_id), pane_id)
}

/// `set-pane-status`.
pub fn set_pane_status(host: &impl IpcHost, pane_id: PaneId, waiting: bool) -> IpcResponse {
    found_or_not(host.set_pane_status(pane_id, waiting), pane_id)
}

/// `mark-completed`, `bell` and `mark-started`: each raises or clears a pane
/// flag without writing to its tty.
pub fn pane_signal(host: &impl IpcHost, pane_id: PaneId, signal: PaneSignal) -> IpcResponse {
    found_or_not(host.pane_signal(pane_id, signal), pane_id)
}

/// `notify`: post a desktop notification whose click focuses `pane_id`.
///
/// Nothing is checked about `pane_id` here: the pane may legitimately be gone by
/// the time the user clicks, and that case is reported then, not now.
pub fn notify(
    host: &impl IpcHost,
    pane_id: Option<PaneId>,
    title: &str,
    message: &str,
    sound: bool,
) -> IpcResponse {
    match host.notify(pane_id, title, message, sound) {
        Ok(()) => IpcResponse::Ok { data: None },
        Err(e) => IpcResponse::Error { message: e },
    }
}

/// `list-tabs`: every tab of the host.
pub fn list_tabs(host: &impl IpcHost) -> IpcResponse {
    let mut tabs = Vec::new();
    host.collect_tabs(&mut tabs);
    IpcResponse::Ok {
        data: Some(serde_json::Value::Array(tabs.iter().map(tab_json).collect())),
    }
}

/// `close-tab`. The host refuses to close the last tab of a window (would
/// terminate the app — use a dedicated shutdown path for that).
pub fn close_tab(host: &impl IpcHost, tab_id: u32) -> IpcResponse {
    match host.close_tab(tab_id) {
        IpcCloseTabResult::Closed => IpcResponse::Ok { data: None },
        IpcCloseTabResult::WouldTerminate => IpcResponse::Error {
            message: format!("tab {} is the last tab in its window — refusing to close (would terminate app)", tab_id),
        },
        IpcCloseTabResult::NotFound => IpcResponse::Error { message: format!("tab {} not found", tab_id) },
    }
}

/// Translate an IPC mode string into a `DumpMode`. Caller has already validated
/// the value, but we map defensively to keep the boundary explicit.
pub fn parse_dump_mode(mode: &str) -> DumpMode {
    match mode {
        "scrollback" => DumpMode::Scrollback,
        "all" => DumpMode::All,
        _ => DumpMode::Visible,
    }
}

/// Resolve a `PaneFilter` into the concrete list of pane IDs to act on.
/// For `All`, every pane of the host in order; for `Ids`, the list as-is.
fn resolve_pane_filter(host: &impl IpcHost, filter: PaneFilter) -> Vec<PaneId> {
    match filter {
        PaneFilter::Ids(ids) => ids,
        PaneFilter::All => {
            let mut ids = Vec::new();
            host.collect_pane_ids(&mut ids);
            ids
        }
    }
}

/// `get-pane-content`: the rendered text of the requested panes.
pub fn get_pane_content(host: &impl IpcHost, filter: PaneFilter, mode_str: &str, trim: bool) -> IpcResponse {
    let mode = parse_dump_mode(mode_str);
    let ids = resolve_pane_filter(host, filter);
    pane_content_response(ids, |pane_id| host.dump_pane_text(pane_id, mode, trim))
}

/// `count-pane-content`: only the size (chars/bytes) of what `get-pane-content`
/// would return.
pub fn count_pane_content(host: &impl IpcHost, filter: PaneFilter, mode_str: &str, trim: bool) -> IpcResponse {
    let mode = parse_dump_mode(mode_str);
    let ids = resolve_pane_filter(host, filter);
    pane_count_response(ids, |pane_id| host.measure_pane_text(pane_id, mode, trim))
}

/// The answer to a wait whose pane is marked completed.
fn completed(pane_id: PaneId) -> IpcResponse {
    IpcResponse::Ok {
        data: Some(serde_json::json!({
            "completed": true,
            "pane_id": pane_id,
            "timed_out": false,
        })),
    }
}

/// `wait-for-completion`: wait for a pane to be marked completed (OSC 133;D or
/// `mark-completed`).
///
/// If the flag is already set when the request arrives, reply immediately
/// (don't make the client wait an extra tick for the obvious answer).
/// Otherwise return `Pending` so the main thread keeps polling on each tick.
pub fn wait_for_completion(host: &impl IpcHost, pane_id: PaneId, timeout_ms: u64) -> Disposition {
    match host.check_completion(pane_id) {
        Some(true) => Disposition::Reply(completed(pane_id)),
        Some(false) => {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
            Disposition::Pending(DeferredWait { pane_id, deadline })
        }
        None => Disposition::Reply(not_found(pane_id)),
    }
}

/// On each main-thread tick, resolve any `wait-for-completion` requests
/// whose pane was marked completed, hit their deadline, or got closed.
pub fn poll_pending_waits(host: &impl IpcHost, waits: &mut Vec<PendingWait>) {
    if waits.is_empty() {
        return;
    }

    let now = std::time::Instant::now();

    waits.retain(|wait| {
        let completion = match host.check_completion(wait.pane_id) {
            Some(c) => c,
            None => {
                let _ = wait.response_tx.send(IpcResponse::Error {
                    message: format!("pane {} closed during wait", wait.pane_id),
                });
                return false;
            }
        };

        if completion {
            let _ = wait.response_tx.send(completed(wait.pane_id));
            return false;
        }

        if now >= wait.deadline {
            let _ = wait.response_tx.send(IpcResponse::Ok {
                data: Some(serde_json::json!({
                    "completed": false,
                    "pane_id": wait.pane_id,
                    "timed_out": true,
                })),
            });
            return false;
        }

        true // still waiting
    });
}

// ─── What a command does to one pane ────────────────────────────────────────

/// What a `send-keys` to this pane would type into.
pub fn typing_target_of(surface: &Surface) -> crate::typed_keys::Target {
    let agent = surface.agent_kind();
    crate::typed_keys::Target {
        agent: agent.map(|a| a.as_str().to_string()),
        // A `claude` process that has not reported its session yet already
        // reads what is typed.
        claude: agent == Some(crate::agent_session::Agent::Claude)
            || (agent.is_none() && surface.fg_process().is_some_and(|p| p.name == "claude")),
        session: surface.agent_session_name(),
        session_id: surface.agent_session_id(),
    }
}

/// `send-keys` on one pane: write `text` to its PTY as is.
pub fn send_keys_to(surface: &Surface, text: &str) {
    // Same rule as a keystroke: someone answered this pane.
    surface.clear_awaiting();
    // A pane still asleep since a relaunch is being used by whoever sends
    // this: its resume line must not be typed over their command once shown.
    surface.cancel_sleep();
    // A submitted line starts a command or a turn: the finished
    // flag of the last one must not answer the wait that usually
    // follows. Cleared here rather than on the shell's 133;C or
    // Claude's UserPromptSubmit hook, both of which can arrive
    // after that wait does.
    if text.contains(['\r', '\n']) {
        surface.terminal.read().mark_command_started();
    }
    surface.pty.write(text.as_bytes());
}

/// The JSON shape of one pane, shared by `list-panes`, the `subscribe` snapshot
/// and the event payloads — a client parses the same object everywhere, from
/// Kova.app (`crate::window::pane_json`) or from a library host. `title` is
/// the host's display title for it, `minimized` its layout state.
///
/// Costs a `proc_pidinfo` (the CWD) and a process-table walk (the children), so
/// callers on a polling path must build it only for panes they actually report.
pub fn surface_json(
    pane: &Surface,
    title: &str,
    minimized: bool,
    win_idx: usize,
    tab_idx: usize,
    focused: bool,
) -> serde_json::Value {
    let pid = pane.pty.pid();
    let children = pane.pty.child_processes();
    let is_idle = children.is_empty();
    let child_json: Vec<serde_json::Value> = children
        .into_iter()
        .map(|(cpid, info)| {
            serde_json::json!({
                "pid": cpid,
                "name": info.name,
                "version": info.version,
            })
        })
        .collect();
    serde_json::json!({
        "id": pane.id,
        "window": win_idx,
        "tab": tab_idx,
        "cwd": pane.cwd().unwrap_or_default(),
        "title": title,
        "focused": focused,
        "pid": pid,
        "child_processes": child_json,
        "is_idle": is_idle,
        "working": pane.is_working(),
        "awaiting": pane.is_awaiting(),
        "awaiting_since": pane.awaiting_since(),
        // Whether the waiting pane has been looked at here since it started waiting. A remote
        // client walking waiting panes needs that halt, or it hands back panes already read on
        // this Mac. False on a pane that is not waiting at all, where the bit means nothing.
        "awaiting_seen": pane.is_awaiting() && !pane.is_awaiting_unseen(),
        "minimized": minimized,
        // Which agent holds the pane's conversation ("claude", "codex", "pi"),
        // absent at a bare shell. `claude_session_id` stays Claude-only so a
        // client that resumes with it never gets an id Claude cannot open; the
        // agent-agnostic id is next to it.
        "agent": pane.agent_kind().map(|a| a.as_str()),
        "agent_session_id": pane.agent_session_id(),
        "agent_session_name": pane.agent_session_name(),
        "claude_session_id": pane.claude_session_id(),
        "claude_session_name": pane.claude_session_name(),
        // The CLAUDE_CONFIG_DIR the session runs under when it is not
        // ~/.claude: its transcript is under <dir>/projects, not ~/.claude/projects.
        "claude_config_dir": pane.claude_config_dir(),
        // A conversation a launch restore left asleep here: its resume line
        // waits until the pane is on screen or `wake-pane`. No agent runs, so
        // the session ids above are null and it has no `peer_name`.
        "dormant_session_id": pane.dormant_session_id(),
        // The SendMessage / ListAgents address of the pane's Claude session,
        // derived names included — `claude_session_name` only carries /rename.
        "peer_name": pane.claude_peer_name(),
    })
}

/// One `get-pane-content` entry for a pane.
pub fn pane_text_entry(pane_id: PaneId, surface: &Surface, mode: DumpMode, trim: bool) -> serde_json::Value {
    let term = surface.terminal.read();
    let dump = term.dump_text(mode, trim);
    serde_json::json!({
        "id": pane_id,
        "text": dump.text,
        "cols": dump.cols,
        "rows": dump.rows,
        "cursor": { "row": dump.cursor_row, "col": dump.cursor_col },
    })
}

/// `(chars, bytes)` of the text `pane_text_entry` would return.
pub fn measure_pane_text(surface: &Surface, mode: DumpMode, trim: bool) -> (usize, usize) {
    surface.terminal.read().measure_text(mode, trim)
}

/// The OSC 133;D flag of a pane.
pub fn completion_flag(surface: &Surface) -> bool {
    surface
        .terminal
        .read()
        .command_completed
        .load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    //! The generic commands driven through `dispatch` against a fake host, with
    //! the exact line the connection thread would write back.
    use super::*;
    use crate::ipc::parse_command;
    use std::cell::RefCell;

    /// A host with panes 1 and 2 (pane 2 finished) and tabs 10 and 11. Pane 3
    /// is the refused last pane, tab 11 the refused last tab. Records what the
    /// commands asked it to do.
    #[derive(Default)]
    struct FakeHost {
        calls: RefCell<Vec<String>>,
        notify_error: Option<String>,
    }

    impl FakeHost {
        fn log(&self, call: String) {
            self.calls.borrow_mut().push(call);
        }
        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    const PANES: [PaneId; 2] = [1, 2];

    impl IpcHost for FakeHost {
        fn collect_panes(&self, out: &mut Vec<serde_json::Value>) {
            for id in PANES {
                out.push(serde_json::json!({"id": id, "title": format!("p{id}")}));
            }
        }
        fn collect_pane_ids(&self, out: &mut Vec<PaneId>) {
            out.extend(PANES);
        }
        fn send_keys(&self, pane_id: PaneId, text: &str) -> bool {
            self.log(format!("send_keys {pane_id} {text:?}"));
            PANES.contains(&pane_id)
        }
        fn typing_target(&self, pane_id: PaneId) -> Option<crate::typed_keys::Target> {
            // Pane 1 runs Claude Code as session `chief`, pane 2 is a shell.
            PANES.contains(&pane_id).then(|| crate::typed_keys::Target {
                agent: (pane_id == 1).then(|| "claude".to_string()),
                claude: pane_id == 1,
                session: (pane_id == 1).then(|| "chief".to_string()),
                session_id: (pane_id == 1).then(|| "s-1".to_string()),
            })
        }
        fn dump_pane_text(&self, pane_id: PaneId, mode: DumpMode, trim: bool) -> Option<serde_json::Value> {
            self.log(format!("dump {pane_id} {mode:?} {trim}"));
            PANES.contains(&pane_id).then(|| serde_json::json!({"id": pane_id, "text": "hi\n"}))
        }
        fn measure_pane_text(&self, pane_id: PaneId, mode: DumpMode, trim: bool) -> Option<(usize, usize)> {
            self.log(format!("measure {pane_id} {mode:?} {trim}"));
            PANES.contains(&pane_id).then_some((3, 4))
        }
        fn check_completion(&self, pane_id: PaneId) -> Option<bool> {
            PANES.contains(&pane_id).then_some(pane_id == 2)
        }
        fn focus_pane(&self, pane_id: PaneId) -> bool {
            self.log(format!("focus {pane_id}"));
            PANES.contains(&pane_id)
        }
        fn close_pane(&self, pane_id: PaneId) -> Option<bool> {
            self.log(format!("close_pane {pane_id}"));
            match pane_id {
                1 | 2 => Some(true),
                3 => Some(false),
                _ => None,
            }
        }
        fn set_pane_status(&self, pane_id: PaneId, waiting: bool) -> bool {
            self.log(format!("status {pane_id} {waiting}"));
            PANES.contains(&pane_id)
        }
        fn pane_signal(&self, pane_id: PaneId, signal: PaneSignal) -> bool {
            self.log(format!("signal {pane_id} {signal:?}"));
            PANES.contains(&pane_id)
        }
        fn notify(&self, pane_id: Option<PaneId>, title: &str, message: &str, sound: bool) -> Result<(), String> {
            self.log(format!("notify {pane_id:?} {title} {message} {sound}"));
            match &self.notify_error {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }
        fn collect_tabs(&self, out: &mut Vec<TabInfo>) {
            for (i, id) in [10u32, 11].into_iter().enumerate() {
                out.push(TabInfo {
                    id,
                    window: 0,
                    tab_index: i,
                    title: format!("t{id}"),
                    pane_count: 1,
                    focused_pane_id: PANES[i],
                    active: i == 0,
                    has_bell: i == 1,
                    has_completion: false,
                    has_running: true,
                });
            }
        }
        fn close_tab(&self, tab_id: u32) -> IpcCloseTabResult {
            self.log(format!("close_tab {tab_id}"));
            match tab_id {
                10 => IpcCloseTabResult::Closed,
                11 => IpcCloseTabResult::WouldTerminate,
                _ => IpcCloseTabResult::NotFound,
            }
        }
    }

    /// The line the connection thread writes for `request`, which must be a
    /// host command answered at once.
    fn reply(host: &FakeHost, request: &str) -> String {
        let cmd = parse_command(request).unwrap_or_else(|e| panic!("{request}: {e}"));
        match dispatch(host, cmd) {
            Ok(Disposition::Reply(r)) => r.to_json().to_string(),
            Ok(Disposition::Pending(_)) => panic!("{request}: pending"),
            Err(_) => panic!("{request}: not a host command"),
        }
    }

    #[test]
    fn list_panes_returns_the_host_entries_as_an_array() {
        let host = FakeHost::default();
        assert_eq!(
            reply(&host, r#"{"cmd":"list-panes"}"#),
            r#"{"data":[{"id":1,"title":"p1"},{"id":2,"title":"p2"}],"ok":true}"#
        );
    }

    #[test]
    fn send_keys_found_and_missing() {
        let host = FakeHost::default();
        assert_eq!(reply(&host, r#"{"cmd":"send-keys","pane_id":2,"text":"ls\r"}"#), r#"{"ok":true}"#);
        assert_eq!(
            reply(&host, r#"{"cmd":"send-keys","pane_id":9,"text":"x"}"#),
            r#"{"error":"pane 9 not found","ok":false}"#
        );
        assert_eq!(host.calls(), ["send_keys 2 \"ls\\r\""], "a pane that does not exist is never written to");
    }

    /// The three cases of the tag rule, replayed against the journal: a tagged
    /// line, a bare line to Claude Code, a shell command to a shell.
    #[test]
    fn send_keys_journals_every_send_and_holds_untagged_text_to_claude_code() {
        use crate::typed_keys::{Caller, Mode, Policy, REFUSAL};
        let dir = std::env::temp_dir().join(format!("kova-send-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let journal = dir.join("send-keys.jsonl");
        let caller = Caller { pid: Some(4242), name: Some("python3".into()) };
        let ok = IpcResponse::Ok { data: None }.to_json().to_string();
        let send = |host: &FakeHost, mode: Mode, pane: PaneId, text: &str| {
            let policy = Policy { mode, journal: Some(journal.clone()) };
            send_keys_under(host, pane, text, &caller, &policy).to_json().to_string()
        };
        let lines = || -> Vec<serde_json::Value> {
            std::fs::read_to_string(&journal)
                .unwrap_or_default()
                .lines()
                .map(|l| serde_json::from_str(l).expect("one JSON object per line"))
                .collect()
        };
        const CLAUDE: PaneId = 1;
        const SHELL: PaneId = 2;

        // 1. A tagged line to Claude Code: typed and journaled, in every mode.
        let host = FakeHost::default();
        for mode in [Mode::Off, Mode::Warn, Mode::Enforce] {
            assert_eq!(send(&host, mode, CLAUDE, "[gardien] reprends"), ok);
        }
        assert_eq!(host.calls().len(), 3);
        let l = lines();
        assert_eq!(l.len(), 3);
        assert_eq!(l[2]["verdict"], "accepted");
        assert_eq!(l[2]["kind"], "tagged");
        assert_eq!(l[2]["tag"], "gardien");
        assert_eq!(l[2]["mode"], "enforce");
        assert_eq!(l[2]["pane_id"], 1);
        assert_eq!(l[2]["session"], "chief");
        assert_eq!(l[2]["agent"], "claude");
        assert_eq!(l[2]["caller_pid"], 4242);
        assert_eq!(l[2]["sha256"], crate::typed_keys::sha256_hex(b"[gardien] reprends"));

        // 2. A bare line to Claude Code: typed and flagged in warn, refused
        // and not typed in enforce, plain in off.
        let host = FakeHost::default();
        assert_eq!(send(&host, Mode::Warn, CLAUDE, "ok envoie"), ok);
        assert_eq!(host.calls(), ["send_keys 1 \"ok envoie\""]);
        let refused = send(&host, Mode::Enforce, CLAUDE, "ok envoie");
        assert_eq!(refused, IpcResponse::Error { message: REFUSAL.to_string() }.to_json().to_string());
        assert_eq!(host.calls().len(), 1, "a refused text never reaches the pane");
        assert_eq!(send(&host, Mode::Off, CLAUDE, "ok envoie"), ok);
        let l = lines();
        let verdicts: Vec<&str> = l[3..].iter().map(|e| e["verdict"].as_str().unwrap()).collect();
        assert_eq!(verdicts, ["would_refuse", "refused", "accepted"]);
        assert!(l[3..].iter().all(|e| e["kind"] == "untagged" && e["tag"].is_null()));
        assert!(!std::fs::read_to_string(&journal).unwrap().contains("envoie"), "the text itself is never journaled");

        // 3. A shell command to a pane without Claude Code: typed, in enforce too.
        let host = FakeHost::default();
        assert_eq!(send(&host, Mode::Enforce, SHELL, "claude --resume abc\r"), ok);
        assert_eq!(host.calls().len(), 1);
        let l = lines();
        let last = l.last().unwrap();
        assert_eq!((last["verdict"].as_str(), last["kind"].as_str(), last["claude"].as_bool()), (Some("accepted"), Some("untagged"), Some(false)));
        assert!(last["agent"].is_null());

        // Always typed into Claude Code: keys alone and `/` lines. Both are
        // journaled: an Esc or a Ctrl+U after a tagged line must leave a trace.
        let host = FakeHost::default();
        for keys in ["\r", "\u{1b}", "\u{15}", "/rename lead"] {
            assert_eq!(send(&host, Mode::Enforce, CLAUDE, keys), ok, "{keys:?}");
        }
        assert_eq!(host.calls().len(), 4);
        let l = lines();
        let tail: Vec<(&str, bool)> = l[l.len() - 4..].iter().map(|e| (e["kind"].as_str().unwrap(), e["edits"].as_bool().unwrap())).collect();
        assert_eq!(tail, [("control", false), ("control", true), ("control", true), ("slash", false)]);
        assert_eq!(l.last().unwrap()["slash"], "/rename");

        // A pane that does not exist: an error, nothing typed, nothing journaled.
        let before = lines().len();
        assert_eq!(send(&host, Mode::Enforce, 9, "x"), r#"{"error":"pane 9 not found","ok":false}"#);
        assert_eq!(lines().len(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn focus_pane_found_and_missing() {
        let host = FakeHost::default();
        assert_eq!(reply(&host, r#"{"cmd":"focus-pane","pane_id":1}"#), r#"{"ok":true}"#);
        assert_eq!(reply(&host, r#"{"cmd":"focus-pane","pane_id":9}"#), r#"{"error":"pane 9 not found","ok":false}"#);
        assert_eq!(focus_pane(&host, 1).to_json().to_string(), r#"{"ok":true}"#, "the notification click path");
    }

    #[test]
    fn close_pane_closed_refused_and_missing() {
        let host = FakeHost::default();
        assert_eq!(reply(&host, r#"{"cmd":"close-pane","pane_id":1}"#), r#"{"ok":true}"#);
        assert_eq!(
            reply(&host, r#"{"cmd":"close-pane","pane_id":3}"#),
            r#"{"error":"pane 3 is the last pane — cannot close","ok":false}"#
        );
        assert_eq!(reply(&host, r#"{"cmd":"close-pane","pane_id":9}"#), r#"{"error":"pane 9 not found","ok":false}"#);
    }

    #[test]
    fn get_pane_content_expands_all_and_keeps_misses() {
        let host = FakeHost::default();
        assert_eq!(
            reply(&host, r#"{"cmd":"get-pane-content"}"#),
            r#"{"data":{"panes":[{"id":1,"text":"hi\n"},{"id":2,"text":"hi\n"}]},"ok":true}"#
        );
        assert_eq!(
            reply(&host, r#"{"cmd":"get-pane-content","panes":[2,9],"mode":"scrollback","trim_trailing_blank_lines":false}"#),
            r#"{"data":{"panes":[{"id":2,"text":"hi\n"},{"error":"not found","id":9}]},"ok":true}"#
        );
        assert_eq!(
            host.calls(),
            ["dump 1 Visible true", "dump 2 Visible true", "dump 2 Scrollback false", "dump 9 Scrollback false"]
        );
    }

    #[test]
    fn count_pane_content_totals_the_panes_found() {
        let host = FakeHost::default();
        assert_eq!(
            reply(&host, r#"{"cmd":"count-pane-content","panes":[1,9,2],"mode":"all"}"#),
            r#"{"data":{"panes":[{"bytes":4,"chars":3,"id":1},{"error":"not found","id":9},{"bytes":4,"chars":3,"id":2}],"total_bytes":8,"total_chars":6},"ok":true}"#
        );
        assert_eq!(host.calls(), ["measure 1 All true", "measure 9 All true", "measure 2 All true"]);
    }

    #[test]
    fn wait_for_completion_replies_at_once_waits_or_misses() {
        let host = FakeHost::default();
        assert_eq!(
            reply(&host, r#"{"cmd":"wait-for-completion","pane_id":2}"#),
            r#"{"data":{"completed":true,"pane_id":2,"timed_out":false},"ok":true}"#
        );
        assert_eq!(
            reply(&host, r#"{"cmd":"wait-for-completion","pane_id":9}"#),
            r#"{"error":"pane 9 not found","ok":false}"#
        );
        let before = std::time::Instant::now();
        let cmd = parse_command(r#"{"cmd":"wait-for-completion","pane_id":1,"timeout_ms":5000}"#).unwrap();
        match dispatch(&host, cmd) {
            Ok(Disposition::Pending(wait)) => {
                assert_eq!(wait.pane_id, 1);
                assert!(wait.deadline >= before + std::time::Duration::from_millis(5000));
                assert!(wait.deadline <= std::time::Instant::now() + std::time::Duration::from_millis(5000));
            }
            _ => panic!("an unfinished pane is a pending wait"),
        }
    }

    #[test]
    fn pending_waits_resolve_on_completion_deadline_or_close() {
        let host = FakeHost::default();
        let now = std::time::Instant::now();
        let far = now + std::time::Duration::from_secs(60);
        let past = now - std::time::Duration::from_millis(1);
        let mut rxs = Vec::new();
        let mut waits: Vec<PendingWait> = [(1, far), (2, far), (1, past), (9, far)]
            .into_iter()
            .map(|(pane_id, deadline)| {
                let (tx, rx) = std::sync::mpsc::channel();
                rxs.push(rx);
                PendingWait { pane_id, response_tx: tx, deadline }
            })
            .collect();
        poll_pending_waits(&host, &mut waits);
        assert_eq!(waits.len(), 1, "only the unfinished pane before its deadline keeps waiting");
        assert_eq!(waits[0].pane_id, 1);
        assert!(rxs[0].try_recv().is_err());
        let line = |i: usize| rxs[i].try_recv().unwrap().to_json().to_string();
        assert_eq!(line(1), r#"{"data":{"completed":true,"pane_id":2,"timed_out":false},"ok":true}"#);
        assert_eq!(line(2), r#"{"data":{"completed":false,"pane_id":1,"timed_out":true},"ok":true}"#);
        assert_eq!(line(3), r#"{"error":"pane 9 closed during wait","ok":false}"#);
        let mut none: Vec<PendingWait> = Vec::new();
        poll_pending_waits(&host, &mut none);
    }

    #[test]
    fn list_tabs_serializes_every_field() {
        let host = FakeHost::default();
        assert_eq!(
            reply(&host, r#"{"cmd":"list-tabs"}"#),
            concat!(
                r#"{"data":["#,
                r#"{"active":true,"focused_pane_id":1,"has_bell":false,"has_completion":false,"has_running":true,"id":10,"pane_count":1,"tab_index":0,"title":"t10","window":0},"#,
                r#"{"active":false,"focused_pane_id":2,"has_bell":true,"has_completion":false,"has_running":true,"id":11,"pane_count":1,"tab_index":1,"title":"t11","window":0}"#,
                r#"],"ok":true}"#
            )
        );
    }

    #[test]
    fn close_tab_closed_refused_and_missing() {
        let host = FakeHost::default();
        assert_eq!(reply(&host, r#"{"cmd":"close-tab","tab_id":10}"#), r#"{"ok":true}"#);
        assert_eq!(
            reply(&host, r#"{"cmd":"close-tab","tab_id":11}"#),
            r#"{"error":"tab 11 is the last tab in its window — refusing to close (would terminate app)","ok":false}"#
        );
        assert_eq!(reply(&host, r#"{"cmd":"close-tab","tab_id":12}"#), r#"{"error":"tab 12 not found","ok":false}"#);
    }

    #[test]
    fn pane_status_and_signals_reach_the_host() {
        let host = FakeHost::default();
        for request in [
            r#"{"cmd":"set-pane-status","pane_id":1,"status":"waiting"}"#,
            r#"{"cmd":"set-pane-status","pane_id":1,"status":"none"}"#,
            r#"{"cmd":"mark-completed","pane_id":2}"#,
            r#"{"cmd":"bell","pane_id":2}"#,
            r#"{"cmd":"mark-started","pane_id":2}"#,
        ] {
            assert_eq!(reply(&host, request), r#"{"ok":true}"#, "{request}");
        }
        for request in [
            r#"{"cmd":"set-pane-status","pane_id":9,"status":"waiting"}"#,
            r#"{"cmd":"mark-completed","pane_id":9}"#,
            r#"{"cmd":"bell","pane_id":9}"#,
            r#"{"cmd":"mark-started","pane_id":9}"#,
        ] {
            assert_eq!(reply(&host, request), r#"{"error":"pane 9 not found","ok":false}"#, "{request}");
        }
        assert_eq!(
            host.calls()[..5],
            ["status 1 true", "status 1 false", "signal 2 Completed", "signal 2 Bell", "signal 2 Started"]
        );
    }

    #[test]
    fn notify_passes_through_and_reports_the_host_error() {
        let host = FakeHost::default();
        assert_eq!(reply(&host, r#"{"cmd":"notify","message":"done"}"#), r#"{"ok":true}"#);
        assert_eq!(
            reply(&host, r#"{"cmd":"notify","pane_id":7,"title":"Claude","message":"m","sound":true}"#),
            r#"{"ok":true}"#
        );
        assert_eq!(host.calls(), ["notify None Kova done false", "notify Some(7) Claude m true"]);
        let failing = FakeHost { notify_error: Some("no permission".into()), ..Default::default() };
        assert_eq!(
            reply(&failing, r#"{"cmd":"notify","message":"x"}"#),
            r#"{"error":"no permission","ok":false}"#
        );
    }

    #[test]
    fn app_commands_are_handed_back_untouched() {
        let host = FakeHost::default();
        for request in [
            r#"{"cmd":"split"}"#,
            r#"{"cmd":"new-tab"}"#,
            r#"{"cmd":"set-tab-title","pane_id":1,"title":"x"}"#,
            r#"{"cmd":"set-tab-color","pane_id":1,"color":2}"#,
            r#"{"cmd":"merge-tab","source_tab_id":1,"target_tab_id":2}"#,
            r#"{"cmd":"swap-pane","pane_id_a":1,"pane_id_b":2}"#,
            r#"{"cmd":"resize-pane","pane_id":1,"direction":"grow"}"#,
            r#"{"cmd":"rename-pane","pane_id":1,"title":"x"}"#,
            r#"{"cmd":"dispatch-action","action":"new-tab"}"#,
            r#"{"cmd":"merge-window","source_window":0,"target_window":1}"#,
            r#"{"cmd":"set-anchor","pane_id":1}"#,
            r#"{"cmd":"clear-anchors"}"#,
            r#"{"cmd":"reload-config"}"#,
            r#"{"cmd":"subscribe"}"#,
            r#"{"cmd":"list-subjects"}"#,
            r#"{"cmd":"set-pane-subject","pane_id":1,"subject":"x"}"#,
            r#"{"cmd":"rename-subject","tab_id":1,"name":"x","new_name":"y"}"#,
            r#"{"cmd":"delete-subject","tab_id":1,"name":"x"}"#,
            r#"{"cmd":"set-subject-color","tab_id":1,"name":"x","color":0}"#,
        ] {
            let cmd = parse_command(request).unwrap();
            assert!(dispatch(&host, cmd).is_err(), "{request} belongs to the app");
        }
        assert!(host.calls().is_empty(), "the host was not touched");
        // The value comes back as it went in.
        let cmd = parse_command(r#"{"cmd":"set-tab-color","pane_id":4,"color":3}"#).unwrap();
        assert!(matches!(
            dispatch(&host, cmd),
            Err(IpcCommand::SetTabColor { pane_id: 4, color: Some(3) })
        ));
    }

    #[test]
    fn dump_mode_names() {
        assert!(matches!(parse_dump_mode("visible"), DumpMode::Visible));
        assert!(matches!(parse_dump_mode("scrollback"), DumpMode::Scrollback));
        assert!(matches!(parse_dump_mode("all"), DumpMode::All));
        assert!(matches!(parse_dump_mode("anything"), DumpMode::Visible));
    }
}
