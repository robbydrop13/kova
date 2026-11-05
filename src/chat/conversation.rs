//! A transcript's lines turned into what the chat shows: the user's
//! messages, what the assistant wrote, its tool calls with their results.
//!
//! Built line by line as the transcript grows (`Conversation::apply`), on the
//! rules of KovaLink's daemon (`link/daemon/src/transcript/jsonl.ts`):
//! - the `assistant` lines of one API request share a `requestId` and carry
//!   one block each; `apiBlockIndex` orders them, not the order of the lines
//!   nor their timestamp (several share a millisecond);
//! - a `user` line whose blocks are all `tool_result` is the end of a tool
//!   call, joined to it by `tool_use_id`, not a message;
//! - a message the user typed while the agent worked has no `user` line: it
//!   is an `attachment` of type `queued_command`;
//! - the harness speaks to the model as `user` too (`isMeta`, a non-human
//!   `origin`, a `<system-reminder>`…): never shown as the user's words;
//! - any other line type (a dozen are seen, the list is open) is ignored, as
//!   is a sidechain (a sub-agent's turn).
//!
//! A large tool result is moved by Claude Code to a file the transcript does
//! not reference: what the line holds is all there is, shown truncated.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::tail::RawLine;

/// Longest tool result kept per call, in chars: it is a preview to unfold,
/// the terminal keeps the rest.
pub const RESULT_MAX: usize = 4000;
/// Longest target shown on a tool line, in chars.
const TARGET_MAX: usize = 120;

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// What the user wrote.
    User { id: String, text: String, ts: Option<u64> },
    /// One API request of the assistant: its blocks in `apiBlockIndex` order.
    Assistant { id: String, blocks: Vec<Block>, ts: Option<u64> },
    /// A short line from the harness worth seeing: a slash command, an
    /// interruption, a compaction.
    Note { id: String, text: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    /// (`apiBlockIndex`, position in its line): the order inside a request.
    pub key: (i64, usize),
    pub kind: BlockKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockKind {
    Text(String),
    Thinking,
    Image,
    Tool(Tool),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tool {
    pub id: String,
    pub name: String,
    /// What the line says, KovaLink's way: `Read  app/src/boot.ts`.
    pub verb: String,
    pub target: String,
    /// `+12 -3` for an edit.
    pub detail: Option<String>,
    pub result: Option<ToolResult>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub text: String,
    pub is_error: bool,
    pub truncated: bool,
}

#[derive(Default)]
pub struct Conversation {
    pub items: Vec<Item>,
    by_request: HashMap<String, usize>,
    /// Tool use id -> (item, block), to join a result to its call.
    tools: HashMap<String, (usize, usize)>,
    /// Results that came before their call (never seen, but cheap to allow).
    early_results: HashMap<String, ToolResult>,
    seen: HashSet<String>,
    /// Epoch seconds of the last line that carried a timestamp.
    pub last_ts: Option<u64>,
    /// Bumped on every change, so the view knows when to render again.
    pub revision: u64,
}

impl Conversation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply_all(&mut self, lines: &[RawLine]) {
        for line in lines {
            self.apply(line);
        }
    }

    pub fn apply(&mut self, line: &RawLine) {
        let v = &line.value;
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return;
        }
        // A line read twice (a window that overlaps) changes nothing.
        if let Some(uuid) = v.get("uuid").and_then(Value::as_str)
            && !self.seen.insert(uuid.to_string())
        {
            return;
        }
        let ts = v.get("timestamp").and_then(Value::as_str).and_then(parse_ts);
        let changed = match v.get("type").and_then(Value::as_str) {
            Some("assistant") => self.apply_assistant(line, ts),
            Some("user") => self.apply_user(line, ts),
            Some("attachment") => self.apply_queued(line, ts),
            _ => false,
        };
        if changed {
            if ts.is_some() {
                self.last_ts = ts;
            }
            self.revision += 1;
        }
    }

    fn apply_assistant(&mut self, line: &RawLine, ts: Option<u64>) -> bool {
        let v = &line.value;
        let Some(content) = v.pointer("/message/content") else { return false };
        let key = v
            .get("requestId")
            .and_then(Value::as_str)
            .or_else(|| v.pointer("/message/id").and_then(Value::as_str))
            .or_else(|| v.get("uuid").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| format!("a{}", line.offset));
        let index = match self.by_request.get(&key) {
            Some(&i) => i,
            None => {
                self.items.push(Item::Assistant { id: key.clone(), blocks: Vec::new(), ts });
                self.by_request.insert(key, self.items.len() - 1);
                self.items.len() - 1
            }
        };
        let Item::Assistant { blocks, ts: item_ts, .. } = &mut self.items[index] else { return false };
        if ts.is_some() {
            *item_ts = ts;
        }
        let api = v.get("apiBlockIndex").and_then(Value::as_i64);
        let mut added = Vec::new();
        for (j, raw) in content_blocks(content).into_iter().enumerate() {
            let Some(kind) = assistant_block(raw) else { continue };
            // Without an index, a block goes after what the request holds.
            let key = (api.unwrap_or(blocks.len() as i64), j);
            let at = blocks.partition_point(|b| b.key <= key);
            if let BlockKind::Tool(t) = &kind {
                added.push((t.id.clone(), key));
            }
            blocks.insert(at, Block { key, kind });
        }
        // An insert shifts the blocks after it: re-index this request's tools.
        for (b, block) in blocks.iter().enumerate() {
            if let BlockKind::Tool(t) = &block.kind {
                self.tools.insert(t.id.clone(), (index, b));
            }
        }
        for (id, _) in added {
            if let Some(result) = self.early_results.remove(&id) {
                self.set_result(&id, result);
            }
        }
        true
    }

    fn apply_user(&mut self, line: &RawLine, ts: Option<u64>) -> bool {
        let v = &line.value;
        let Some(content) = v.pointer("/message/content") else { return false };
        let mut changed = false;
        let mut texts = Vec::new();
        let mut images = 0;
        for raw in content_blocks(content) {
            if let Value::String(s) = raw {
                texts.push(s.clone());
                continue;
            }
            match raw.get("type").and_then(Value::as_str) {
                Some("tool_result") => {
                    let id = raw.get("tool_use_id").and_then(Value::as_str).unwrap_or_default().to_string();
                    let (text, truncated) = preview(raw.get("content").unwrap_or(&Value::Null), RESULT_MAX);
                    let result = ToolResult { text, is_error: raw.get("is_error").and_then(Value::as_bool) == Some(true), truncated };
                    if self.tools.contains_key(&id) {
                        self.set_result(&id, result);
                    } else {
                        self.early_results.insert(id, result);
                    }
                    changed = true;
                }
                Some("text") => texts.push(raw.get("text").and_then(Value::as_str).unwrap_or_default().to_string()),
                Some("image") => images += 1,
                _ => {}
            }
        }
        let text = texts.join("\n");
        if text.trim().is_empty() && images == 0 {
            return changed;
        }
        let id = v.get("uuid").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("u{}", line.offset));
        match classify_user(v, &text) {
            UserLine::Human => {
                let mut text = text.trim().to_string();
                for _ in 0..images {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str("[image]");
                }
                self.items.push(Item::User { id, text, ts });
            }
            UserLine::Note(note) => self.items.push(Item::Note { id, text: note }),
            UserLine::Harness => return changed,
        }
        true
    }

    /// A message the user sent while the agent worked.
    fn apply_queued(&mut self, line: &RawLine, ts: Option<u64>) -> bool {
        let v = &line.value;
        let Some(a) = v.get("attachment") else { return false };
        if a.get("type").and_then(Value::as_str) != Some("queued_command") {
            return false;
        }
        let human = a.pointer("/origin/kind").and_then(Value::as_str) == Some("human")
            || a.get("commandMode").and_then(Value::as_str) == Some("prompt");
        if !human {
            return false;
        }
        let text = match a.get("prompt") {
            Some(Value::String(s)) => s.clone(),
            Some(p @ Value::Array(_)) => content_blocks(p)
                .into_iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        if text.trim().is_empty() {
            return false;
        }
        let id = v.get("uuid").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("q{}", line.offset));
        self.items.push(Item::User { id, text: text.trim().to_string(), ts });
        true
    }

    fn set_result(&mut self, tool_id: &str, result: ToolResult) {
        let Some(&(i, b)) = self.tools.get(tool_id) else { return };
        if let Some(Item::Assistant { blocks, .. }) = self.items.get_mut(i)
            && let Some(Block { kind: BlockKind::Tool(t), .. }) = blocks.get_mut(b)
        {
            t.result = Some(result);
        }
    }

    /// When the user last wrote, epoch seconds.
    pub fn last_user_ts(&self) -> Option<u64> {
        self.items.iter().rev().find_map(|i| match i {
            Item::User { ts, .. } => Some(*ts),
            _ => None,
        })?
    }
}

/// `content` as a list of blocks: a plain string is one text block.
fn content_blocks(content: &Value) -> Vec<&Value> {
    match content {
        Value::Array(list) => list.iter().filter(|b| b.is_object()).collect(),
        Value::String(_) => vec![content],
        _ => Vec::new(),
    }
}

fn assistant_block(raw: &Value) -> Option<BlockKind> {
    if let Value::String(s) = raw {
        return (!s.trim().is_empty()).then(|| BlockKind::Text(s.clone()));
    }
    match raw.get("type").and_then(Value::as_str)? {
        "text" => {
            let text = raw.get("text").and_then(Value::as_str).unwrap_or_default();
            (!text.trim().is_empty()).then(|| BlockKind::Text(text.to_string()))
        }
        "thinking" | "redacted_thinking" => Some(BlockKind::Thinking),
        "image" => Some(BlockKind::Image),
        "tool_use" | "server_tool_use" => {
            let name = raw.get("name").and_then(Value::as_str).unwrap_or("tool").to_string();
            let input = raw.get("input").unwrap_or(&Value::Null);
            let (verb, target, detail) = describe_tool(&name, input);
            Some(BlockKind::Tool(Tool {
                id: raw.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                name,
                verb,
                target,
                detail,
                result: None,
            }))
        }
        // An unknown block type is ignored, never an error.
        _ => None,
    }
}

/// The tags the harness speaks through under the `user` role (KovaLink's
/// closed table, `HARNESS_TAGS`).
const HARNESS_TAGS: &[&str] = &[
    "task-notification",
    "system-reminder",
    "system-notification",
    "cross-session-message",
    "local-command-stdout",
    "local-command-stderr",
    "local-command-caveat",
    "command-name",
    "command-message",
    "command-args",
    "bash-input",
    "bash-stdout",
    "bash-stderr",
    "ide_opened_file",
    "ide_selection",
];

enum UserLine {
    Human,
    Note(String),
    Harness,
}

fn classify_user(v: &Value, text: &str) -> UserLine {
    let trimmed = text.trim_start();
    if v.get("isCompactSummary").and_then(Value::as_bool) == Some(true) {
        return UserLine::Note("Conversation compacted".to_string());
    }
    if trimmed.starts_with("[Request interrupted") {
        return UserLine::Note("Interrupted".to_string());
    }
    // A slash command the user typed: the harness wraps it, the name is worth a line.
    if let Some(name) = between(trimmed, "<command-name>", "</command-name>") {
        let args = between(trimmed, "<command-args>", "</command-args>").unwrap_or_default();
        let line = format!("{} {}", name.trim(), args.trim());
        return UserLine::Note(one_line(line.trim(), TARGET_MAX));
    }
    let tag = trimmed.strip_prefix('<').map(|rest| rest.split(|c: char| c == '>' || c.is_whitespace()).next().unwrap_or(""));
    let foreign = v.pointer("/origin/kind").and_then(Value::as_str).is_some_and(|k| k != "human");
    let meta = v.get("isMeta").and_then(Value::as_bool) == Some(true)
        || v.get("promptSource").and_then(Value::as_str) == Some("system");
    if meta || foreign || tag.is_some_and(|t| HARNESS_TAGS.contains(&t)) {
        return UserLine::Harness;
    }
    UserLine::Human
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = text[start..].find(close)? + start;
    Some(&text[start..end])
}

/// A tool call as one line: a verb, a target, a detail. KovaLink's wording
/// (`Lit`, `Execute`, `Modifie`, `Cherche`) in Kova's language, English.
pub fn describe_tool(name: &str, input: &Value) -> (String, String, Option<String>) {
    let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or_default();
    let path = |k: &str| short_path(s(k));
    let lines = |k: &str| s(k).lines().count();
    let (verb, target, detail): (&str, String, Option<String>) = match name {
        "Read" => ("Read", path("file_path"), None),
        "Bash" => ("Run", s("command").to_string(), None),
        "Edit" => ("Edit", path("file_path"), Some(format!("+{} -{}", lines("new_string"), lines("old_string")))),
        "MultiEdit" => ("Edit", path("file_path"), None),
        "Write" => ("Write", path("file_path"), Some(format!("+{}", lines("content")))),
        "NotebookEdit" => ("Edit", path("notebook_path"), None),
        "Grep" => ("Search", format!("\"{}\"", s("pattern")), None),
        "Glob" => ("Find", s("pattern").to_string(), None),
        "WebFetch" => ("Fetch", s("url").to_string(), None),
        "WebSearch" => ("Search the web", s("query").to_string(), None),
        "Task" | "Agent" => ("Agent", s("description").to_string(), None),
        "TodoWrite" => ("Update the to-do list", String::new(), None),
        "Skill" => ("Skill", s("skill").to_string(), None),
        "ToolSearch" => ("Load tools", s("query").to_string(), None),
        _ => {
            // `mcp__server__tool` reads as `tool`, its first text argument as the target.
            let short = name.rsplit("__").next().unwrap_or(name);
            let first = input
                .as_object()
                .and_then(|o| o.values().find_map(Value::as_str))
                .unwrap_or_default();
            return (short.replace('_', " "), one_line(first, TARGET_MAX), None);
        }
    };
    (verb.to_string(), one_line(&target, TARGET_MAX), detail)
}

/// A path as the line shows it: `~/` for the home directory, the last three
/// components of a long one.
fn short_path(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let p = if !home.is_empty() && path.starts_with(&home) { format!("~{}", &path[home.len()..]) } else { path.to_string() };
    let parts: Vec<&str> = p.split('/').collect();
    if parts.len() > 4 { format!("…/{}", parts[parts.len() - 3..].join("/")) } else { p }
}

/// The first non-empty line of `text`, at most `max` chars.
fn one_line(text: &str, max: usize) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    truncate(line, max).0
}

fn truncate(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text.to_string(), false);
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    (out, true)
}

/// A tool result's text, KovaLink's generic preview: a string, the `text`
/// of a list of blocks, or an object's first text field. No exhaustive match
/// on the tools: a new one must not break anything.
pub fn preview(value: &Value, max: usize) -> (String, bool) {
    let text = match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(list) => list
            .iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                Value::Object(o) if o.get("type").and_then(Value::as_str) == Some("image") => "[image]".to_string(),
                Value::Object(o) => o.get("text").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| v.to_string()),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(o) => ["stdout", "text", "content", "output", "result"]
            .iter()
            .find_map(|k| o.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()))
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string()),
        other => other.to_string(),
    };
    truncate(text.trim_end(), max)
}

/// `2026-11-05T10:12:33.123Z` (or with a `±HH:MM` offset) as epoch seconds.
pub fn parse_ts(s: &str) -> Option<u64> {
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    if s.len() < 19 || s.as_bytes()[4] != b'-' || s.as_bytes()[10] != b'T' {
        return None;
    }
    let (y, mo, d, h, mi, se) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut rest = &s[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        rest = frac.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match rest.as_bytes().first() {
        None | Some(b'Z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let oh = rest.get(1..3)?.parse::<i64>().ok()?;
            let om = rest.get(4..6)?.parse::<i64>().ok()?;
            let o = oh * 3600 + om * 60;
            if *sign == b'+' { o } else { -o }
        }
        _ => return None,
    };
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se - offset;
    u64::try_from(secs).ok()
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(offset: u64, value: Value) -> RawLine {
        RawLine { offset, value }
    }

    fn conv(lines: Vec<Value>) -> Conversation {
        let mut c = Conversation::new();
        for (i, v) in lines.into_iter().enumerate() {
            c.apply(&line(i as u64 * 100, v));
        }
        c
    }

    fn asst(req: &str, index: i64, block: Value) -> Value {
        json!({"type": "assistant", "requestId": req, "apiBlockIndex": index, "uuid": format!("{req}-{index}"),
               "message": {"role": "assistant", "content": [block]}})
    }

    fn block_texts(item: &Item) -> Vec<String> {
        let Item::Assistant { blocks, .. } = item else { panic!("not an assistant item: {item:?}") };
        blocks
            .iter()
            .map(|b| match &b.kind {
                BlockKind::Text(t) => t.clone(),
                BlockKind::Thinking => "thinking".into(),
                BlockKind::Image => "image".into(),
                BlockKind::Tool(t) => format!("{} {}", t.verb, t.target),
            })
            .collect()
    }

    #[test]
    fn a_request_s_blocks_follow_api_block_index_not_line_order() {
        let c = conv(vec![
            asst("r1", 2, json!({"type": "text", "text": "third"})),
            asst("r1", 0, json!({"type": "thinking", "thinking": ""})),
            asst("r1", 1, json!({"type": "text", "text": "second"})),
        ]);
        assert_eq!(c.items.len(), 1, "one request, one item");
        assert_eq!(block_texts(&c.items[0]), ["thinking", "second", "third"]);
    }

    #[test]
    fn a_tool_result_joins_its_call_even_after_a_reorder() {
        let c = conv(vec![
            asst("r1", 1, json!({"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "/x/a.rs"}})),
            asst("r1", 0, json!({"type": "text", "text": "Let me look."})),
            json!({"type": "user", "uuid": "u2", "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "fn main() {}"}]}}),
        ]);
        assert_eq!(c.items.len(), 1, "a tool result is not a message");
        let Item::Assistant { blocks, .. } = &c.items[0] else { panic!() };
        let BlockKind::Tool(t) = &blocks[1].kind else { panic!("{:?}", blocks) };
        assert_eq!(t.verb, "Read");
        assert_eq!(t.result.as_ref().map(|r| r.text.as_str()), Some("fn main() {}"));
    }

    #[test]
    fn the_harness_never_speaks_as_the_user() {
        let c = conv(vec![
            json!({"type": "user", "uuid": "a", "message": {"content": "<system-reminder>x</system-reminder>"}}),
            json!({"type": "user", "uuid": "b", "isMeta": true, "message": {"content": "Caveat"}}),
            json!({"type": "user", "uuid": "c", "origin": {"kind": "task"}, "message": {"content": "done"}}),
            json!({"type": "user", "uuid": "d", "message": {"content": "<command-name>/clear</command-name><command-args></command-args>"}}),
            json!({"type": "user", "uuid": "e", "message": {"content": [{"type": "text", "text": "fix the bug"}]}}),
            json!({"type": "user", "uuid": "f", "message": {"content": "[Request interrupted by user]"}}),
        ]);
        assert_eq!(c.items.len(), 3);
        assert_eq!(c.items[0], Item::Note { id: "d".into(), text: "/clear".into() });
        assert!(matches!(&c.items[1], Item::User { text, .. } if text == "fix the bug"));
        assert!(matches!(&c.items[2], Item::Note { text, .. } if text == "Interrupted"));
    }

    #[test]
    fn a_message_sent_mid_turn_is_the_user_s() {
        let c = conv(vec![
            json!({"type": "attachment", "uuid": "q", "attachment": {"type": "queued_command", "prompt": "also run the tests", "commandMode": "prompt"}}),
            json!({"type": "attachment", "uuid": "r", "attachment": {"type": "queued_command", "prompt": "agent done", "commandMode": "task-notification"}}),
        ]);
        assert_eq!(c.items.len(), 1);
        assert!(matches!(&c.items[0], Item::User { text, .. } if text == "also run the tests"));
    }

    #[test]
    fn sidechains_unknown_types_and_repeats_change_nothing() {
        let mut c = conv(vec![
            json!({"type": "assistant", "isSidechain": true, "requestId": "s", "message": {"content": [{"type": "text", "text": "sub"}]}}),
            json!({"type": "permission-mode", "permissionMode": "auto"}),
            json!({"type": "assistant", "requestId": "r", "uuid": "x", "message": {"content": [{"type": "mystery"}]}}),
        ]);
        let before = c.revision;
        c.apply(&line(1, asst("r2", 0, json!({"type": "text", "text": "hi"}))));
        c.apply(&line(2, asst("r2", 0, json!({"type": "text", "text": "hi"}))));
        assert_eq!(c.revision, before + 1, "the second copy of a line is ignored");
        assert_eq!(c.items.len(), 2);
    }

    #[test]
    fn tool_lines_read_like_kovalink_s() {
        assert_eq!(describe_tool("Bash", &json!({"command": "npm test\n--watch"})), ("Run".into(), "npm test".into(), None));
        assert_eq!(
            describe_tool("Edit", &json!({"file_path": "/a/b.rs", "old_string": "x\ny", "new_string": "z"})),
            ("Edit".into(), "/a/b.rs".into(), Some("+1 -2".into()))
        );
        assert_eq!(describe_tool("Grep", &json!({"pattern": "promptHash"})).1, "\"promptHash\"");
        assert_eq!(describe_tool("mcp__linear__get_issue", &json!({"id": "CLA-1"})), ("get issue".into(), "CLA-1".into(), None));
        assert_eq!(short_path("/a/b/c/d/e.rs"), "…/c/d/e.rs");
    }

    #[test]
    fn a_long_result_is_cut_and_says_so() {
        let (text, cut) = preview(&json!("x".repeat(10)), 4);
        assert_eq!((text.as_str(), cut), ("xxxx…", true));
        assert_eq!(preview(&json!([{"type": "text", "text": "a"}, {"type": "image"}]), 100).0, "a\n[image]");
        assert_eq!(preview(&json!({"stdout": "ok", "stderr": ""}), 100).0, "ok");
    }

    #[test]
    fn timestamps_parse_to_epoch_seconds() {
        assert_eq!(parse_ts("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_ts("2026-11-05T10:12:33.123Z"), Some(1_793_873_553));
        assert_eq!(parse_ts("2026-11-05T11:12:33+01:00"), Some(1_793_873_553));
        assert_eq!(parse_ts("yesterday"), None);
    }
}
