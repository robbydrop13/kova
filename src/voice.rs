//! Voice input for the chat's box: the user speaks, AssemblyAI's streaming
//! API (Universal-Streaming v3, French) writes it as it goes, the box fills.
//!
//! This half is pure and tested: where the key comes from and what its
//! absence says, the socket's URL, the server's messages, the transcript
//! built from them, and when a pause sends the message by itself. The audio
//! and the socket are `voice_io`.
//!
//! The key is read from the macOS Keychain only (service
//! `assemblyai-api-key`), never written anywhere, never logged. It only
//! mints a short-lived token; the socket URL carries the token, not the key.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

/// The Keychain item that holds the key.
pub const KEYCHAIN_SERVICE: &str = "assemblyai-api-key";
/// The line the chat shows when there is none.
pub const MISSING_KEY: &str = "Clé AssemblyAI absente du Trousseau";
/// Where a temporary streaming token is minted (POST, the key in `Authorization`).
pub const TOKEN_URL: &str = "https://token.assemblyai.com/v1/token";
/// The token's body: good for one session opened within a minute.
pub const TOKEN_BODY: &str = r#"{"product":"streaming","expires_in_seconds":60}"#;
const WS_URL: &str = "wss://streaming.assemblyai.com/v3/ws";
/// The multilingual model, held to French (measured 06/11/2026: French turns,
/// formatted, on this model with `language_codes=["fr"]`).
const SPEECH_MODEL: &str = "universal-3-6-pro";
pub const SAMPLE_RATE: u32 = 16_000;
/// A silence this long after a finished turn sends the message, when the
/// user asked for it (`[chat] voice_auto_send`).
pub const AUTO_SEND_PAUSE_MS: u64 = 2_000;

static AUTO_SEND: AtomicBool = AtomicBool::new(false);

/// `[chat] voice_auto_send`, set at load and on a config reload.
pub fn set_auto_send(on: bool) {
    AUTO_SEND.store(on, Ordering::Relaxed);
}

pub fn auto_send() -> bool {
    AUTO_SEND.load(Ordering::Relaxed)
}

/// The key `lookup` found, trimmed, or the line that says it is missing.
pub fn api_key(lookup: impl FnOnce() -> Option<String>) -> Result<String, &'static str> {
    match lookup() {
        Some(key) if !key.trim().is_empty() => Ok(key.trim().to_string()),
        _ => Err(MISSING_KEY),
    }
}

/// The socket's URL for a temporary `token`.
pub fn ws_url(token: &str) -> String {
    format!(
        "{WS_URL}?sample_rate={SAMPLE_RATE}&encoding=pcm_s16le&speech_model={SPEECH_MODEL}&language_codes=%5B%22fr%22%5D&token={}",
        encode(token)
    )
}

/// Percent-encoding of everything but the unreserved characters.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The token in the mint endpoint's answer.
pub fn token_from(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    v.get("token").and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string)
}

/// What the server says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Begin,
    /// A turn's text so far; `end` once the turn is final.
    Turn { order: u64, text: String, end: bool },
    Termination,
    Error(String),
    /// Anything else (`SpeechStarted`, a heartbeat, a new type).
    Other,
}

pub fn parse(message: &str) -> Event {
    let Ok(v) = serde_json::from_str::<Value>(message) else { return Event::Other };
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return Event::Error(e.to_string());
    }
    match v.get("type").and_then(Value::as_str) {
        Some("Begin") => Event::Begin,
        Some("Termination") => Event::Termination,
        Some("Turn") => Event::Turn {
            order: v.get("turn_order").and_then(Value::as_u64).unwrap_or(0),
            text: v.get("transcript").and_then(Value::as_str).unwrap_or_default().trim().to_string(),
            end: v.get("end_of_turn").and_then(Value::as_bool).unwrap_or(false),
        },
        _ => Event::Other,
    }
}

/// What was said since the mic went on (or since the last message sent):
/// the finished turns and the one in progress, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Transcript {
    turns: BTreeMap<u64, (String, bool)>,
    /// Turns up to this order went with a message already sent.
    sent_upto: Option<u64>,
    /// When the text last changed, ms on any monotonic clock.
    changed_at: u64,
}

impl Transcript {
    /// Apply a server event at `now`; whether the text changed.
    pub fn apply(&mut self, event: &Event, now: u64) -> bool {
        let Event::Turn { order, text, end } = event else { return false };
        if self.sent_upto.is_some_and(|s| *order <= s) {
            return false;
        }
        let entry = (text.clone(), *end);
        if self.turns.get(order) == Some(&entry) {
            return false;
        }
        let text_changed = self.turns.get(order).is_none_or(|(t, _)| t != text);
        self.turns.insert(*order, entry);
        if text_changed || *end {
            self.changed_at = now;
        }
        text_changed
    }

    pub fn text(&self) -> String {
        self.turns.values().map(|(t, _)| t.as_str()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ")
    }

    /// Every turn heard is final.
    pub fn settled(&self) -> bool {
        self.turns.values().all(|(_, end)| *end)
    }

    /// Whether a pause after the last finished turn sends the message now.
    pub fn auto_send_due(&self, now: u64, pause_ms: u64) -> bool {
        !self.text().is_empty() && self.settled() && now.saturating_sub(self.changed_at) >= pause_ms
    }

    /// The message went: start again from what comes next.
    pub fn clear(&mut self) {
        self.sent_upto = self.turns.keys().next_back().copied().or(self.sent_upto);
        self.turns.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_or_empty_key_says_so_and_never_reaches_the_socket() {
        assert_eq!(api_key(|| None), Err(MISSING_KEY));
        assert_eq!(api_key(|| Some("  \n".into())), Err(MISSING_KEY));
        assert_eq!(api_key(|| Some("fake-key\n".into())).as_deref(), Ok("fake-key"));
        let url = ws_url("a+b/c=");
        assert!(url.ends_with("&token=a%2Bb%2Fc%3D"));
        assert!(url.contains("speech_model=universal-3-6-pro") && url.contains("language_codes=%5B%22fr%22%5D"));
        assert!(url.contains("sample_rate=16000&encoding=pcm_s16le"));
        assert_eq!(token_from(r#"{"token":"t1","expires_in_seconds":60}"#).as_deref(), Some("t1"));
        assert_eq!(token_from(r#"{"error":"Invalid API key"}"#), None);
    }

    #[test]
    fn server_messages_parse_into_partial_and_final_turns() {
        // As measured on the live API, words left out.
        let partial = r#"{"turn_order":0,"turn_is_formatted":true,"end_of_turn":false,"transcript":"Bonjour, plus","type":"Turn"}"#;
        let fin = r#"{"turn_order":0,"turn_is_formatted":true,"end_of_turn":true,"transcript":"Bonjour, peux-tu relancer les tests ?","type":"Turn"}"#;
        assert_eq!(parse(partial), Event::Turn { order: 0, text: "Bonjour, plus".into(), end: false });
        assert_eq!(parse(fin), Event::Turn { order: 0, text: "Bonjour, peux-tu relancer les tests ?".into(), end: true });
        assert_eq!(parse(r#"{"type":"Begin","id":"x","expires_at":1}"#), Event::Begin);
        assert_eq!(parse(r#"{"type":"Termination","audio_duration_seconds":7}"#), Event::Termination);
        assert_eq!(parse(r#"{"type":"SpeechStarted","timestamp":0}"#), Event::Other);
        assert_eq!(parse(r#"{"error":"Unauthorized"}"#), Event::Error("Unauthorized".into()));
        assert_eq!(parse("not json"), Event::Other);
    }

    #[test]
    fn the_transcript_keeps_final_turns_and_replaces_the_partial_one() {
        let mut t = Transcript::default();
        assert!(t.apply(&Event::Turn { order: 0, text: "Bonjour".into(), end: false }, 0));
        assert!(t.apply(&Event::Turn { order: 0, text: "Bonjour à toi.".into(), end: true }, 100));
        assert!(t.apply(&Event::Turn { order: 1, text: "Relance".into(), end: false }, 200));
        assert_eq!(t.text(), "Bonjour à toi. Relance");
        assert!(!t.settled());
        assert!(!t.apply(&Event::Begin, 300));
        t.clear();
        assert_eq!(t.text(), "");
        assert!(!t.apply(&Event::Turn { order: 1, text: "Relance les tests.".into(), end: true }, 400), "a turn that went with the message stays out");
        assert!(t.apply(&Event::Turn { order: 2, text: "Merci.".into(), end: false }, 500));
        assert_eq!(t.text(), "Merci.");
    }

    #[test]
    fn a_pause_after_a_finished_turn_sends_by_itself() {
        let mut t = Transcript::default();
        assert!(!t.auto_send_due(10_000, AUTO_SEND_PAUSE_MS), "nothing said");
        t.apply(&Event::Turn { order: 0, text: "Envoie".into(), end: false }, 1_000);
        assert!(!t.auto_send_due(9_000, AUTO_SEND_PAUSE_MS), "a turn still open is not a pause");
        t.apply(&Event::Turn { order: 0, text: "Envoie.".into(), end: true }, 1_500);
        assert!(!t.auto_send_due(3_000, AUTO_SEND_PAUSE_MS), "1.5 s is not enough");
        assert!(t.auto_send_due(3_500, AUTO_SEND_PAUSE_MS));
        t.apply(&Event::Turn { order: 1, text: "Et".into(), end: false }, 3_600);
        assert!(!t.auto_send_due(9_000, AUTO_SEND_PAUSE_MS), "the user spoke again");
    }
}
