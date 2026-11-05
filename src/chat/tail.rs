//! A Claude transcript (`~/.claude/projects/<slug>/<id>.jsonl`) read as it
//! grows: a window from the end at open, then only the bytes added since.
//!
//! Claude Code appends whole lines, but a read can land between the bytes
//! of one: a line without its `\n` is kept aside and parsed once the rest
//! arrives. A file rewritten in place (`/clear`, a resume) is seen by its
//! inode, its size going down, or its first bytes changing, and read again
//! from the end (`Read::reopened`), the way KovaLink's daemon tails it.

use std::io::{Read as _, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// What a chat reads at open, from the end. A line can weigh 130 KB, and a
/// long session several hundred MB: the window is capped, the rest stays
/// in the terminal's scrollback.
pub const MAX_INITIAL_BYTES: u64 = 4 * 1024 * 1024;
/// The fingerprint that tells a file rewritten in place from one that grew.
const HEAD_BYTES: usize = 64;

/// One complete line of the transcript, parsed.
#[derive(Clone, Debug, PartialEq)]
pub struct RawLine {
    /// Byte offset of the line in the file: stable across reads.
    pub offset: u64,
    pub value: serde_json::Value,
}

/// What a read brought.
#[derive(Debug, Default)]
pub struct Read {
    pub lines: Vec<RawLine>,
    /// The file was rewritten: `lines` replace everything read before.
    pub reopened: bool,
}

pub struct Tail {
    path: PathBuf,
    /// The next byte to read.
    offset: u64,
    /// 0 while the file does not exist: Claude Code writes it on the first message.
    inode: u64,
    head: Vec<u8>,
    /// A line still missing its `\n`, and where it starts in the file.
    carry: Vec<u8>,
    carry_start: u64,
}

impl Tail {
    /// Open `path` and read its last `MAX_INITIAL_BYTES`. A missing file is an
    /// empty conversation, not an error: it is opened once it exists.
    pub fn open(path: &Path) -> (Tail, Vec<RawLine>) {
        let mut tail = Tail { path: path.to_path_buf(), offset: 0, inode: 0, head: Vec::new(), carry: Vec::new(), carry_start: 0 };
        let lines = tail.read_window();
        (tail, lines)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The lines added since the last read.
    pub fn read_more(&mut self) -> Read {
        let Ok(meta) = std::fs::metadata(&self.path) else { return Read::default() };
        if meta.ino() != self.inode || meta.len() < self.offset || !self.head_matches() {
            let lines = self.read_window();
            return Read { lines, reopened: true };
        }
        if meta.len() == self.offset {
            return Read::default();
        }
        let Some(bytes) = read_range(&self.path, self.offset, meta.len()) else { return Read::default() };
        self.offset += bytes.len() as u64;
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(&bytes);
        let (lines, rest) = split_lines(&buf, self.carry_start);
        self.carry_start += rest as u64;
        self.carry = buf[rest..].to_vec();
        if self.head.len() < HEAD_BYTES {
            self.head = read_range(&self.path, 0, HEAD_BYTES as u64).unwrap_or_default();
        }
        Read { lines, reopened: false }
    }

    fn head_matches(&self) -> bool {
        if self.head.is_empty() {
            return true;
        }
        read_range(&self.path, 0, self.head.len() as u64).is_some_and(|now| now == self.head)
    }

    /// Read the file's last window again, from scratch.
    fn read_window(&mut self) -> Vec<RawLine> {
        self.carry.clear();
        let Ok(meta) = std::fs::metadata(&self.path) else {
            self.inode = 0;
            self.offset = 0;
            self.carry_start = 0;
            self.head.clear();
            return Vec::new();
        };
        let size = meta.len();
        self.inode = meta.ino();
        self.head = read_range(&self.path, 0, HEAD_BYTES as u64).unwrap_or_default();
        let start = size.saturating_sub(MAX_INITIAL_BYTES);
        let bytes = read_range(&self.path, start, size).unwrap_or_default();
        self.offset = start + bytes.len() as u64;
        // A window that starts mid-file starts mid-line: that first piece is dropped.
        let skip = if start > 0 { bytes.iter().position(|&b| b == b'\n').map_or(bytes.len(), |i| i + 1) } else { 0 };
        let (lines, rest) = split_lines(&bytes[skip..], start + skip as u64);
        self.carry_start = start + (skip + rest) as u64;
        self.carry = bytes[skip + rest..].to_vec();
        lines
    }
}

/// The complete lines of `bytes`, read at file offset `base`, and the index
/// where the incomplete rest starts. A line that is not a JSON object with a
/// `type` is skipped, never an error.
pub fn split_lines(bytes: &[u8], base: u64) -> (Vec<RawLine>, usize) {
    let mut lines = Vec::new();
    let mut start = 0;
    while let Some(len) = bytes[start..].iter().position(|&b| b == b'\n') {
        let line = &bytes[start..start + len];
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(line)
            && value.get("type").is_some_and(|t| t.is_string())
        {
            lines.push(RawLine { offset: base + start as u64, value });
        }
        start += len + 1;
    }
    (lines, start)
}

fn read_range(path: &Path, start: u64, end: u64) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(end.saturating_sub(start)).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("kova-chat-tail-{}-{}", std::process::id(), crate::ask::new_question_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn append(path: &Path, text: &str) {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    fn types(lines: &[RawLine]) -> Vec<String> {
        lines.iter().map(|l| l["type"].as_str().unwrap().to_string()).collect()
    }

    impl std::ops::Index<&str> for RawLine {
        type Output = serde_json::Value;
        fn index(&self, key: &str) -> &serde_json::Value {
            &self.value[key]
        }
    }

    #[test]
    fn a_partial_last_line_waits_for_its_newline() {
        let path = dir().join("s.jsonl");
        append(&path, "{\"type\":\"user\"}\n{\"type\":\"assis");
        let (mut tail, lines) = Tail::open(&path);
        assert_eq!(types(&lines), ["user"]);
        assert!(tail.read_more().lines.is_empty());
        append(&path, "tant\"}\n");
        let read = tail.read_more();
        assert!(!read.reopened);
        assert_eq!(types(&read.lines), ["assistant"]);
        assert_eq!(read.lines[0].offset, 16, "the offset is where the line starts, carry included");
    }

    #[test]
    fn a_missing_file_is_an_empty_conversation_opened_once_written() {
        let path = dir().join("s.jsonl");
        let (mut tail, lines) = Tail::open(&path);
        assert!(lines.is_empty());
        assert!(tail.read_more().lines.is_empty());
        append(&path, "{\"type\":\"user\"}\n");
        let read = tail.read_more();
        assert_eq!(types(&read.lines), ["user"]);
    }

    #[test]
    fn a_rewritten_file_is_read_again() {
        let path = dir().join("s.jsonl");
        append(&path, "{\"type\":\"user\",\"n\":1}\n{\"type\":\"user\",\"n\":2}\n");
        let (mut tail, _) = Tail::open(&path);
        std::fs::write(&path, "{\"type\":\"user\",\"n\":3}\n").unwrap();
        let read = tail.read_more();
        assert!(read.reopened);
        assert_eq!(read.lines.len(), 1);
        assert_eq!(read.lines[0]["n"], 3);
    }

    #[test]
    fn junk_lines_are_skipped() {
        let (lines, rest) = split_lines(b"not json\n{\"no\":\"type\"}\n{\"type\":\"user\"}\n{\"type\"", 100);
        assert_eq!(types(&lines), ["user"]);
        assert_eq!(lines[0].offset, 100 + 23);
        assert_eq!(rest, 39);
    }
}
