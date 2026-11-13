//! Who typed into a pane through the socket.
//!
//! In an agent's transcript, a line typed by a tool through `send-keys` looks
//! exactly like a line the user typed at the keyboard. The socket is the one
//! place every tool goes through and the keyboard never does, so two things
//! happen here:
//!
//! - every `send-keys` that names an existing pane is appended to a journal
//!   (`~/Library/Logs/Kova/send-keys.jsonl`), without its text: time, pane,
//!   session, a hash, what kind of text it was, who asked;
//! - printable text sent to a pane running Claude Code must start with a
//!   `[name] ` tag saying who types. `[ipc] untagged_send_keys` says what
//!   happens to the rest: `off` (nothing), `warn` (typed, journaled as
//!   `would_refuse`), `enforce` (refused, journaled as `refused`).
//!
//! Always typed: control keys alone, the three commands of `FREE_SLASH`,
//! anything sent to a pane that does not run Claude Code, and a send signed
//! with the user's click key (`Signature`): his own words, relayed by his
//! phone.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// What happens to untagged text sent to a pane running Claude Code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    Warn,
    Enforce,
}

impl Mode {
    /// The `[ipc] untagged_send_keys` value. Anything else is `warn`, said in
    /// the log: a typo must neither switch the check off nor start refusing.
    pub fn from_config(value: &str) -> Mode {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Mode::Off,
            "warn" | "" => Mode::Warn,
            "enforce" => Mode::Enforce,
            other => {
                log::warn!("config: [ipc] untagged_send_keys = {:?} is not off, warn or enforce: using warn", other);
                Mode::Warn
            }
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Warn => "warn",
            Mode::Enforce => "enforce",
        }
    }
}

/// What a `send-keys` text is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// No printable character: Enter, Esc, Ctrl+U, an arrow key.
    Control,
    /// Every piece of text is one of the `FREE_SLASH` commands.
    Slash,
    /// Every piece of text starts with a `[name] ` tag (or is one of the
    /// `FREE_SLASH` commands).
    Tagged,
    /// Some piece of text starts with neither.
    Untagged,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Control => "control",
            Kind::Slash => "slash",
            Kind::Tagged => "tagged",
            Kind::Untagged => "untagged",
        }
    }
}

/// A `send-keys` text, read the way the prompt it lands in will take it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Typed {
    pub kind: Kind,
    /// The name inside the first `[name] ` tag.
    pub tag: Option<String>,
    /// The first `/command` word.
    pub slash: Option<String>,
    /// The text holds a key that edits or leaves the line: anything but
    /// Enter, a newline, a tab and the bracketed-paste marks. A tagged line
    /// followed by Home and four deletes is a tagged line no more; this bit is
    /// what shows it.
    pub edits: bool,
    /// The printable text alone, trimmed: what the prompt shows when this
    /// send is the whole line.
    pub printable: String,
}

/// Longest name a tag may hold, in chars.
const TAG_MAX: usize = 40;

/// The `/` commands typed without a tag: they manage the conversation and
/// ask the agent for nothing. Any other `/` line is a prompt (`/skill do
/// this`) and is judged like text.
pub const FREE_SLASH: [&str; 3] = ["/rename", "/compact", "/clear"];

fn is_free_slash(piece: &str) -> bool {
    piece.split_whitespace().next().is_some_and(|word| FREE_SLASH.contains(&word))
}

/// The name of the `[name] ` tag `piece` starts with.
fn tag_of(piece: &str) -> Option<&str> {
    let rest = piece.strip_prefix('[')?;
    let end = rest.find(']')?;
    let name = &rest[..end];
    let named = !name.trim().is_empty() && name.chars().count() <= TAG_MAX && !name.contains('[');
    (named && rest[end + 1..].starts_with(' ')).then_some(name)
}

/// Read `text` as keys. Each stretch of printable characters between two keys
/// that submit, erase or move (Enter, Ctrl+U, an arrow…) is a piece of its
/// own and must be tagged by itself: `[a] x`, Ctrl+U, `go` types `go`.
pub fn read(text: &str) -> Typed {
    let mut pieces: Vec<String> = vec![String::new()];
    let mut edits = false;
    let mut chars = text.chars().peekable();
    // A key that ends the piece being read.
    let cut = |pieces: &mut Vec<String>| pieces.push(String::new());
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                let mut sequence = String::new();
                match chars.peek().copied() {
                    // CSI: parameters, then one final byte.
                    Some('[') => {
                        chars.next();
                        for p in chars.by_ref() {
                            sequence.push(p);
                            if ('\x40'..='\x7e').contains(&p) {
                                break;
                            }
                        }
                    }
                    // OSC: up to BEL or ESC \.
                    Some(']') => {
                        chars.next();
                        sequence.push(']');
                        while let Some(p) = chars.next() {
                            if p == '\x07' {
                                break;
                            }
                            if p == '\x1b' {
                                chars.next_if_eq(&'\\');
                                break;
                            }
                        }
                    }
                    // SS3 (application cursor keys): one more char.
                    Some('O') => {
                        chars.next();
                        chars.next();
                        sequence.push('O');
                    }
                    // Alt+key, or Esc alone.
                    Some(_) => {
                        chars.next();
                        sequence.push('m');
                    }
                    None => {}
                }
                // The bracketed-paste marks carry text, they are not keys.
                if sequence != "200~" && sequence != "201~" {
                    edits = true;
                    cut(&mut pieces);
                }
            }
            '\r' => cut(&mut pieces),
            '\n' | '\t' => pieces.last_mut().expect("never empty").push(c),
            c if c.is_control() => {
                edits = true;
                cut(&mut pieces);
            }
            c => pieces.last_mut().expect("never empty").push(c),
        }
    }
    let pieces: Vec<&str> = pieces.iter().map(|p| p.trim()).filter(|p| !p.is_empty()).collect();
    let tag = pieces.iter().find_map(|p| tag_of(p)).map(str::to_string);
    let slash = pieces
        .iter()
        .find(|p| p.starts_with('/'))
        .map(|p| p.split_whitespace().next().unwrap_or("/").chars().take(TAG_MAX).collect::<String>());
    let kind = if pieces.is_empty() {
        Kind::Control
    } else if pieces.iter().any(|p| tag_of(p).is_none() && !is_free_slash(p)) {
        Kind::Untagged
    } else if tag.is_some() {
        Kind::Tagged
    } else {
        Kind::Slash
    };
    Typed { kind, tag, slash, edits, printable: pieces.join("\n") }
}

/// What the journal says became of a `send-keys`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Typed.
    Accepted,
    /// Typed, and `enforce` would have refused it.
    WouldRefuse,
    /// Not typed.
    Refused,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Accepted => "accepted",
            Verdict::WouldRefuse => "would_refuse",
            Verdict::Refused => "refused",
        }
    }
}

/// The rule: only untagged text to a pane running Claude Code is ever held,
/// and never when the send carries a good signature.
pub fn verdict(mode: Mode, kind: Kind, claude: bool, signed: bool) -> Verdict {
    match (kind, claude && !signed, mode) {
        (Kind::Untagged, true, Mode::Warn) => Verdict::WouldRefuse,
        (Kind::Untagged, true, Mode::Enforce) => Verdict::Refused,
        _ => Verdict::Accepted,
    }
}

/// What the client of a refused `send-keys` reads.
pub const REFUSAL: &str = "send-keys refused: text typed into a pane running Claude Code must start with a \"[name] \" tag saying who types it, e.g. \"[my-tool] done\". Control keys alone and /rename, /compact, /clear are typed as is. See docs/ipc.md, send-keys";

/// The optional `sig` of a `send-keys`: proof that the text comes from the
/// user through an app holding his click key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    /// Which app signed (`kovalink`): 1 to 40 chars of `a-z`, `0-9`, `-`.
    pub by: String,
    /// When it signed, in milliseconds since the Unix epoch.
    pub ts: u64,
    /// HMAC-SHA256 of `signed_message`, keyed by the click key, lowercase hex.
    pub mac: String,
}

/// A signature is good for this long around its `ts`, and only once.
pub const SIGNATURE_WINDOW_MS: u64 = 30_000;

/// The exact bytes a `send-keys` signature covers: five lines joined by
/// `\n`, no trailing newline. The text enters by its SHA-256 (of the bytes
/// received, UTF-8), so the message is one printable line per part whatever
/// the text holds.
pub fn signed_message(by: &str, pane_id: u32, ts: u64, text: &str) -> String {
    format!("kova-send-keys-v1\n{}\n{}\n{}\n{}", by, pane_id, ts, sha256_hex(text.as_bytes()))
}

/// Why a signature does not count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureError {
    /// Kova has no click key to check it against.
    NoKey,
    /// `ts` is more than `SIGNATURE_WINDOW_MS` away from now.
    Stale,
    /// The MAC is not the one of this pane, text, time and key.
    Bad,
    /// This very signature was already used.
    Replayed,
}

impl SignatureError {
    pub fn as_str(self) -> &'static str {
        match self {
            SignatureError::NoKey => "no_key",
            SignatureError::Stale => "stale",
            SignatureError::Bad => "bad_signature",
            SignatureError::Replayed => "replayed",
        }
    }
}

/// The signatures accepted during the current window: `(ts, mac)`.
pub type Seen = Vec<(u64, String)>;

/// Whether `sig` proves this send. A good one is remembered in `seen` until
/// its window closes, so it cannot type the same text twice.
pub fn check_signature(
    sig: &Signature,
    pane_id: u32,
    text: &str,
    key: Option<&str>,
    now_ms: u64,
    seen: &mut Seen,
) -> Result<(), SignatureError> {
    let key = key.ok_or(SignatureError::NoKey)?;
    if now_ms.abs_diff(sig.ts) > SIGNATURE_WINDOW_MS {
        return Err(SignatureError::Stale);
    }
    let expected = hmac_sha256_hex(key.as_bytes(), signed_message(&sig.by, pane_id, sig.ts, text).as_bytes());
    if !same_bytes(expected.as_bytes(), sig.mac.to_ascii_lowercase().as_bytes()) {
        return Err(SignatureError::Bad);
    }
    seen.retain(|(ts, _)| now_ms.abs_diff(*ts) <= SIGNATURE_WINDOW_MS);
    if seen.iter().any(|(_, mac)| *mac == expected) {
        return Err(SignatureError::Replayed);
    }
    seen.push((sig.ts, expected));
    Ok(())
}

/// Equality that takes the same time wherever the first difference is.
fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

/// What the signature of a send came to, for the journal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Signed {
    /// The app whose signature was good.
    pub by: Option<String>,
    /// Why the signature presented did not count.
    pub error: Option<&'static str>,
}

/// Check the signature of a live `send-keys` against the click key read now
/// and the signatures this process already took.
pub fn signed_now(sig: Option<&Signature>, pane_id: u32, text: &str) -> Signed {
    static SEEN: Mutex<Seen> = Mutex::new(Vec::new());
    let Some(sig) = sig else { return Signed::default() };
    let key = crate::attention::signing_key();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    match check_signature(sig, pane_id, text, key.as_deref(), now_ms, &mut seen) {
        Ok(()) => Signed { by: Some(sig.by.clone()), error: None },
        Err(e) => Signed { by: None, error: Some(e.as_str()) },
    }
}

/// The pane a `send-keys` names, as the journal describes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Target {
    /// The agent holding the pane's conversation (`claude`, `codex`…).
    pub agent: Option<String>,
    /// Claude Code runs here: a Claude conversation, or a `claude` process
    /// that has not reported its session yet.
    pub claude: bool,
    pub session: Option<String>,
    pub session_id: Option<String>,
}

/// Who is at the other end of the connection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Caller {
    pub pid: Option<i32>,
    /// The executable's name, read when the connection opened.
    pub name: Option<String>,
}

impl Caller {
    /// The process holding the other end of `stream` (`LOCAL_PEERPID`).
    pub fn of(stream: &std::os::unix::net::UnixStream) -> Caller {
        use std::os::fd::AsRawFd;
        const SOL_LOCAL: libc::c_int = 0;
        const LOCAL_PEERPID: libc::c_int = 0x002;
        let mut pid: libc::pid_t = 0;
        let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
        // SAFETY: `pid` and `len` describe one pid_t, which is what
        // LOCAL_PEERPID writes; the descriptor is open for the call.
        let got = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                SOL_LOCAL,
                LOCAL_PEERPID,
                (&mut pid as *mut libc::pid_t).cast(),
                &mut len,
            )
        };
        if got != 0 || pid <= 0 {
            return Caller::default();
        }
        Caller { pid: Some(pid), name: process_name(pid) }
    }
}

/// The executable name of `pid`, `None` once it is gone.
fn process_name(pid: i32) -> Option<String> {
    let mut buf = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is ours and its length is passed along.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    let path = String::from_utf8_lossy(&buf[..n as usize]).into_owned();
    Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned())
}

/// What `send-keys` applies: the mode, and the journal it writes to.
#[derive(Debug, Clone)]
pub struct Policy {
    pub mode: Mode,
    /// `None`: nothing is journaled.
    pub journal: Option<PathBuf>,
}

/// The policy in force. Unset (a test, a host that never configured it):
/// `warn`, nothing journaled.
static POLICY: Mutex<Option<Policy>> = Mutex::new(None);

/// Set at launch and on `reload-config`.
pub fn configure(mode: Mode, journal: Option<PathBuf>) {
    log::info!(
        "send-keys: untagged text to a Claude Code pane is on {:?}, journal {}",
        mode.as_str(),
        journal.as_deref().map(|p| p.display().to_string()).unwrap_or_else(|| "off".into())
    );
    *POLICY.lock().unwrap_or_else(|e| e.into_inner()) = Some(Policy { mode, journal });
}

pub fn policy() -> Policy {
    POLICY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or(Policy { mode: Mode::Warn, journal: None })
}

/// `~/Library/Logs/Kova/send-keys.jsonl`, next to `kova.log`.
pub fn default_journal() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("Library/Logs/Kova/send-keys.jsonl")
}

/// Past this size the journal becomes `send-keys.jsonl.1` (the one before it
/// is dropped) and a new one starts.
const JOURNAL_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// One journal line for a `send-keys`: everything but the text.
pub fn entry(
    ts: &str,
    pane_id: u32,
    target: &Target,
    text: &str,
    typed: &Typed,
    mode: Mode,
    verdict: Verdict,
    caller: &Caller,
    signed: &Signed,
) -> serde_json::Value {
    serde_json::json!({
        "ts": ts,
        "pane_id": pane_id,
        "session": target.session,
        "session_id": target.session_id,
        "agent": target.agent,
        "claude": target.claude,
        "kind": typed.kind.as_str(),
        "tag": typed.tag,
        "slash": typed.slash,
        "edits": typed.edits,
        "len": text.len(),
        "chars": text.chars().count(),
        "sha256": sha256_hex(text.as_bytes()),
        "text_sha256": (!typed.printable.is_empty()).then(|| sha256_hex(typed.printable.as_bytes())),
        "mode": mode.as_str(),
        "verdict": verdict.as_str(),
        "signed": signed.by,
        "sig_error": signed.error,
        "caller_pid": caller.pid,
        "caller": caller.name,
    })
}

/// Append one line to the journal. A journal that cannot be written is said
/// once per failure in `kova.log` and never stops the keys.
pub fn append(path: &Path, line: &serde_json::Value) {
    static WRITING: Mutex<()> = Mutex::new(());
    let _one_writer = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    if std::fs::metadata(path).is_ok_and(|m| m.len() > JOURNAL_MAX_BYTES) {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        let _ = std::fs::rename(path, PathBuf::from(old));
    }
    let written = (|| {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        // One write per line: two appends never interleave.
        options.open(path)?.write_all(format!("{}\n", line).as_bytes())
    })();
    if let Err(e) = written {
        log::warn!("send-keys: could not write the journal {}: {}", path.display(), e);
    }
}

/// Now, UTC, to the millisecond: `2026-11-13T08:41:07.123Z`, the shape of a
/// transcript's `timestamp`.
pub fn now_utc() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    utc(now.as_secs() as i64, now.subsec_millis())
}

fn utc(secs: i64, millis: u32) -> String {
    let t: libc::time_t = secs as libc::time_t;
    // SAFETY: `tm` is plain data that gmtime_r fills from `t`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::gmtime_r(&t, &mut tm) };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        millis
    )
}

unsafe extern "C" {
    /// CommonCrypto, part of libSystem.
    fn CC_SHA256(data: *const libc::c_void, len: u32, md: *mut u8) -> *mut u8;
    fn CCHmac(
        algorithm: u32,
        key: *const libc::c_void,
        key_len: usize,
        data: *const libc::c_void,
        data_len: usize,
        mac_out: *mut libc::c_void,
    );
}

/// `kCCHmacAlgSHA256`.
const CC_HMAC_SHA256: u32 = 2;

/// HMAC-SHA256 of `data` under `key`, in lowercase hex.
pub fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> String {
    let mut mac = [0u8; 32];
    // SAFETY: each pointer goes with its own length, and `mac` holds the 32
    // bytes an HMAC-SHA256 writes.
    unsafe {
        CCHmac(CC_HMAC_SHA256, key.as_ptr().cast(), key.len(), data.as_ptr().cast(), data.len(), mac.as_mut_ptr().cast())
    };
    mac.iter().map(|b| format!("{:02x}", b)).collect()
}

/// SHA-256 of `data`, in lowercase hex.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut digest = [0u8; 32];
    // SAFETY: `digest` holds the 32 bytes CC_SHA256 writes; a `send-keys`
    // text is bounded by the socket's line limit, far under u32::MAX.
    unsafe { CC_SHA256(data.as_ptr().cast(), data.len() as u32, digest.as_mut_ptr()) };
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_is_read_as_the_prompt_will_take_it() {
        let t = read("[gardien] Continue ce qui ne dépend pas de Robin.");
        assert_eq!((t.kind, t.tag.as_deref(), t.edits), (Kind::Tagged, Some("gardien"), false));
        assert_eq!(read("[file de Robin] go\r").tag.as_deref(), Some("file de Robin"));
        assert_eq!(read("ok vas-y").kind, Kind::Untagged);
        assert_eq!(read("1\r").kind, Kind::Untagged);
        let slash = read("/rename kova-chat-fix");
        assert_eq!((slash.kind, slash.slash.as_deref()), (Kind::Slash, Some("/rename")));
        // A tag needs a name and the space after it.
        for bare in ["[] go", "[x]go", "[x", " go", "[[x] y] go", &format!("[{}] go", "n".repeat(TAG_MAX + 1))] {
            assert_eq!(read(bare).kind, Kind::Untagged, "{bare:?}");
        }
    }

    #[test]
    fn keys_alone_are_control_and_say_when_they_edit_the_line() {
        for (keys, edits) in [("\r", false), ("\n", false), ("", false), ("\x1b", true), ("\x15", true), ("\x1b[A", true), ("\x1bOA", true), ("\x7f\x7f", true), ("\x03", true)] {
            let t = read(keys);
            assert_eq!((t.kind, t.edits), (Kind::Control, edits), "{keys:?}");
            assert_eq!(t.printable, "");
        }
    }

    #[test]
    fn each_piece_of_text_between_two_keys_must_be_tagged_itself() {
        // The tagged line is erased, then a bare one is typed.
        assert_eq!(read("[x] hello\x15ok go").kind, Kind::Untagged);
        // The tagged line is submitted, then a bare one.
        assert_eq!(read("[x] hello\rok go\r").kind, Kind::Untagged);
        assert_eq!(read("[x] hello\x1b[Hok go").kind, Kind::Untagged);
        assert_eq!(read("/rename a\rok go\r").kind, Kind::Untagged);
        // Only three commands go untagged: any other `/` line is a prompt.
        for free in ["/rename a", "/compact garde ton role", "/clear", "/clear\r"] {
            assert_eq!(read(free).kind, Kind::Slash, "{free:?}");
        }
        for prompt in ["/loop envoie le mail", "/update", "/renamed x", "/ rename x", "/rename a\r/loop go\r"] {
            let t = read(prompt);
            assert_eq!(t.kind, Kind::Untagged, "{prompt:?}");
            assert!(t.slash.is_some(), "the journal still names the command");
        }
        // Two tagged lines, a tagged line then a command: all named.
        assert_eq!(read("[x] one\r[y] two\r").kind, Kind::Tagged);
        assert_eq!(read("[x] one\r/compact\r").kind, Kind::Tagged);
        // A newline stays inside the message it belongs to.
        assert_eq!(read("[x] one\ntwo").kind, Kind::Tagged);
        // A tagged line edited after the fact is still tagged here; `edits`
        // is what the journal keeps of it.
        let edited = read("[x] ok go\x1b[H\x1b[3~\x1b[3~\x1b[3~\x1b[3~");
        assert_eq!((edited.kind, edited.edits), (Kind::Tagged, true));
    }

    #[test]
    fn a_bracketed_paste_is_text_not_keys() {
        let pasted = read("\x1b[200~ok go\x1b[201~");
        assert_eq!((pasted.kind, pasted.edits, pasted.printable.as_str()), (Kind::Untagged, false, "ok go"));
        let tagged = read("\x1b[200~[x] ok\x1b[201~\r");
        assert_eq!((tagged.kind, tagged.edits), (Kind::Tagged, false));
        // An OSC is swallowed whole, its payload is not text.
        assert_eq!(read("\x1b]0;title\x07").kind, Kind::Control);
    }

    #[test]
    fn only_untagged_text_to_a_claude_pane_is_held() {
        for mode in [Mode::Off, Mode::Warn, Mode::Enforce] {
            for kind in [Kind::Control, Kind::Slash, Kind::Tagged] {
                assert_eq!(verdict(mode, kind, true, false), Verdict::Accepted);
            }
            assert_eq!(verdict(mode, Kind::Untagged, false, false), Verdict::Accepted, "a shell takes anything");
            assert_eq!(verdict(mode, Kind::Untagged, true, true), Verdict::Accepted, "a signed send is the user's");
        }
        assert_eq!(verdict(Mode::Off, Kind::Untagged, true, false), Verdict::Accepted);
        assert_eq!(verdict(Mode::Warn, Kind::Untagged, true, false), Verdict::WouldRefuse);
        assert_eq!(verdict(Mode::Enforce, Kind::Untagged, true, false), Verdict::Refused);
    }

    /// A made-up key: the real one is never in a test.
    const TEST_KEY: &str = "test-key-0123456789abcdef";

    fn sign(by: &str, pane_id: u32, ts: u64, text: &str, key: &str) -> Signature {
        Signature { by: by.into(), ts, mac: hmac_sha256_hex(key.as_bytes(), signed_message(by, pane_id, ts, text).as_bytes()) }
    }

    #[test]
    fn the_signed_bytes_and_the_mac_are_the_documented_ones() {
        // RFC 4231, test case 2.
        assert_eq!(
            hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // The vector of docs/ipc.md: another implementation must find the same.
        let message = signed_message("kovalink", 42, 1_794_559_267_123, "ok envoie\r");
        assert_eq!(
            message,
            "kova-send-keys-v1\nkovalink\n42\n1794559267123\n76862c0e9525051b4ae0c88ab25d0ee38e7c5a75e8e78c5d56e445c03618a950"
        );
        assert_eq!(hmac_sha256_hex(TEST_KEY.as_bytes(), message.as_bytes()), "f3f75f48803ed7f589cfd31bce3a396766849a5d667213aec47164031fe407e9");
    }

    #[test]
    fn a_good_signature_counts_once_within_its_window() {
        let now = 1_794_559_267_123u64;
        let mut seen = Seen::new();
        let sig = sign("kovalink", 42, now, "ok envoie\r", TEST_KEY);
        assert_eq!(check_signature(&sig, 42, "ok envoie\r", Some(TEST_KEY), now + 800, &mut seen), Ok(()));
        // The same signature again is a replay.
        assert_eq!(check_signature(&sig, 42, "ok envoie\r", Some(TEST_KEY), now + 900, &mut seen), Err(SignatureError::Replayed));
        // Uppercase hex is the same MAC.
        let upper = Signature { mac: sign("kovalink", 42, now + 1, "x", TEST_KEY).mac.to_uppercase(), ..sign("kovalink", 42, now + 1, "x", TEST_KEY) };
        assert_eq!(check_signature(&upper, 42, "x", Some(TEST_KEY), now, &mut seen), Ok(()));
        // The edge of the window is still inside it, in the past or the future.
        for ts in [now - SIGNATURE_WINDOW_MS, now + SIGNATURE_WINDOW_MS] {
            let edge = sign("kovalink", 42, ts, "edge", TEST_KEY);
            assert_eq!(check_signature(&edge, 42, "edge", Some(TEST_KEY), now, &mut seen), Ok(()));
        }
    }

    #[test]
    fn a_signature_for_something_else_or_by_another_key_is_bad() {
        let now = 1_794_559_267_123u64;
        let mut seen = Seen::new();
        let sig = sign("kovalink", 42, now, "ok envoie\r", TEST_KEY);
        let bad = |pane: u32, text: &str, sig: &Signature, key: &str, seen: &mut Seen| check_signature(sig, pane, text, Some(key), now, seen);
        assert_eq!(bad(43, "ok envoie\r", &sig, TEST_KEY, &mut seen), Err(SignatureError::Bad), "another pane");
        assert_eq!(bad(42, "ok envoie tout\r", &sig, TEST_KEY, &mut seen), Err(SignatureError::Bad), "another text");
        assert_eq!(bad(42, "ok envoie\r", &sig, "another-key-0123456789", &mut seen), Err(SignatureError::Bad), "another key");
        let renamed = Signature { by: "watch".into(), ..sig.clone() };
        assert_eq!(bad(42, "ok envoie\r", &renamed, TEST_KEY, &mut seen), Err(SignatureError::Bad), "another signer");
        let moved = Signature { ts: now + 1, ..sig.clone() };
        assert_eq!(bad(42, "ok envoie\r", &moved, TEST_KEY, &mut seen), Err(SignatureError::Bad), "another time");
        for mac in ["", "00", &"0".repeat(64)] {
            let forged = Signature { mac: mac.to_string(), ..sig.clone() };
            assert_eq!(bad(42, "ok envoie\r", &forged, TEST_KEY, &mut seen), Err(SignatureError::Bad));
        }
        assert!(seen.is_empty(), "a bad signature uses nothing up");
        // None of those spoiled the good one.
        assert_eq!(bad(42, "ok envoie\r", &sig, TEST_KEY, &mut seen), Ok(()));
    }

    #[test]
    fn a_stale_signature_or_one_kova_cannot_check_does_not_count() {
        let now = 1_794_559_267_123u64;
        let mut seen = Seen::new();
        for ts in [now - SIGNATURE_WINDOW_MS - 1, now + SIGNATURE_WINDOW_MS + 1, 0] {
            let old = sign("kovalink", 42, ts, "ok", TEST_KEY);
            assert_eq!(check_signature(&old, 42, "ok", Some(TEST_KEY), now, &mut seen), Err(SignatureError::Stale));
        }
        let sig = sign("kovalink", 42, now, "ok", TEST_KEY);
        assert_eq!(check_signature(&sig, 42, "ok", None, now, &mut seen), Err(SignatureError::NoKey));
        // A signature taken long ago is forgotten, and stale by then anyway.
        assert_eq!(check_signature(&sig, 42, "ok", Some(TEST_KEY), now, &mut seen), Ok(()));
        let later = now + 2 * SIGNATURE_WINDOW_MS;
        let fresh = sign("kovalink", 42, later, "ok", TEST_KEY);
        assert_eq!(check_signature(&fresh, 42, "ok", Some(TEST_KEY), later, &mut seen), Ok(()));
        assert_eq!(seen.len(), 1, "the old one left the memory");
    }

    #[test]
    fn the_setting_reads_three_values_and_a_typo_is_warn() {
        assert_eq!(Mode::from_config("off"), Mode::Off);
        assert_eq!(Mode::from_config(" Enforce "), Mode::Enforce);
        assert_eq!(Mode::from_config("warn"), Mode::Warn);
        assert_eq!(Mode::from_config("enforced"), Mode::Warn);
        assert_eq!(Mode::from_config(""), Mode::Warn);
    }

    #[test]
    fn the_hash_and_the_time_have_the_shape_a_checker_expects() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(utc(0, 7), "1970-01-01T00:00:00.007Z");
        assert_eq!(utc(1_794_559_267, 123), "2026-11-13T08:41:07.123Z");
    }

    #[test]
    fn the_caller_is_the_process_at_the_other_end_of_the_socket() {
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let caller = Caller::of(&ours);
        assert_eq!(caller.pid, Some(std::process::id() as i32));
        assert!(caller.name.is_some_and(|n| n.starts_with("kova")));
        drop(theirs);
    }

    #[test]
    fn a_journal_line_holds_everything_but_the_text() {
        let dir = std::env::temp_dir().join(format!("kova-typed-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("logs/send-keys.jsonl");
        let text = "[gardien] secret words\r";
        let typed = read(text);
        let target = Target { agent: Some("claude".into()), claude: true, session: Some("chief".into()), session_id: Some("abc".into()) };
        let caller = Caller { pid: Some(42), name: Some("python3".into()) };
        let line = entry("2026-11-13T08:41:07.123Z", 7, &target, text, &typed, Mode::Warn, Verdict::Accepted, &caller, &Signed::default());
        append(&path, &line);
        append(&path, &line);
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written.lines().count(), 2);
        assert!(!written.contains("secret"), "{written}");
        let back: serde_json::Value = serde_json::from_str(written.lines().next().unwrap()).unwrap();
        assert_eq!(back["pane_id"], 7);
        assert_eq!(back["session"], "chief");
        assert_eq!(back["tag"], "gardien");
        assert_eq!(back["kind"], "tagged");
        assert_eq!(back["verdict"], "accepted");
        assert_eq!(back["len"], text.len());
        assert_eq!(back["sha256"], sha256_hex(text.as_bytes()));
        assert_eq!(back["text_sha256"], sha256_hex(b"[gardien] secret words"));
        assert!(back["signed"].is_null() && back["sig_error"].is_null());
        assert_eq!(back["caller_pid"], 42);
        assert_eq!(back["caller"], "python3");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
