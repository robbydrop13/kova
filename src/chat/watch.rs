//! What tells the chat that the session moved: a kqueue on the transcript
//! and on the hooks' turn folder, asked without waiting once per frame.
//!
//! The kernel queues the change the moment Claude Code writes, so the chat
//! reads its files only when they changed, one frame later at most, instead
//! of every 200 ms. A file replaced or deleted (a `/clear`, a turn file
//! renamed into place) is a change too; the caller watches again what it
//! opens next. Nothing here blocks: `kevent` is called with a zero timeout.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::RawFd;
use std::path::Path;

pub struct Watch {
    kq: RawFd,
    fds: Vec<RawFd>,
}

impl Watch {
    /// Watch `paths` (files or folders) that exist; the others are skipped.
    /// `None` if the kqueue cannot be made.
    pub fn new(paths: &[&Path]) -> Option<Watch> {
        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            return None;
        }
        let mut watch = Watch { kq, fds: Vec::new() };
        for path in paths {
            let Ok(c) = CString::new(path.as_os_str().as_bytes()) else { continue };
            let fd = unsafe { libc::open(c.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
            if fd < 0 {
                continue;
            }
            let change = libc::kevent {
                ident: fd as libc::uintptr_t,
                filter: libc::EVFILT_VNODE,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: libc::NOTE_WRITE | libc::NOTE_EXTEND | libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_ATTRIB,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            let added = unsafe { libc::kevent(kq, &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
            if added < 0 {
                unsafe { libc::close(fd) };
                continue;
            }
            watch.fds.push(fd);
        }
        Some(watch)
    }

    /// Whether anything watched changed since the last call. Never waits.
    pub fn fired(&self) -> bool {
        let zero = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        let mut events: [libc::kevent; 8] = unsafe { std::mem::zeroed() };
        let mut any = false;
        loop {
            let n = unsafe { libc::kevent(self.kq, std::ptr::null(), 0, events.as_mut_ptr(), events.len() as i32, &zero) };
            if n <= 0 {
                return any;
            }
            any = true;
            if (n as usize) < events.len() {
                return true;
            }
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        for fd in &self.fds {
            unsafe { libc::close(*fd) };
        }
        unsafe { libc::close(self.kq) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn a_write_or_a_file_renamed_into_a_folder_fires_once() {
        let dir = std::env::temp_dir().join(format!("kova-watch-{}", crate::ask::new_question_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("t.jsonl");
        std::fs::write(&file, "a\n").unwrap();
        let w = Watch::new(&[&file, &dir, &dir.join("missing")]).unwrap();
        assert!(!w.fired(), "nothing yet");
        std::fs::OpenOptions::new().append(true).open(&file).unwrap().write_all(b"b\n").unwrap();
        assert!(w.fired(), "an append, seen without waiting");
        assert!(!w.fired(), "drained");
        // A hook's turn file: written aside, renamed into place.
        std::fs::write(dir.join(".x.tmp"), "ready 1").unwrap();
        std::fs::rename(dir.join(".x.tmp"), dir.join("x")).unwrap();
        assert!(w.fired());
        std::fs::remove_dir_all(&dir).ok();
    }
}
