//! Running a local CLI with a deadline.
//!
//! `routinectl` and `team` are local commands, but one that hangs must not
//! leave a background refresh flag set for good, nor a row saying
//! `Opening…` with no outcome ever posted: every child this drives is killed
//! once its deadline passes.

use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// How often the wait looks at the child. Four times a second is plenty for
/// deadlines counted in seconds, and keeps the reaping loop cheap.
const POLL: Duration = Duration::from_millis(250);

/// Run `cmd` to completion, killing it once `deadline` has passed. `Err` is
/// the spawn failure or the timeout, in words fit for a row: a timed-out
/// child is killed and reaped, never left running.
///
/// Its output is read while it runs, on two threads: a child that prints
/// more than a pipe holds (64 KB on macOS) would otherwise block on its
/// write, never exit, and be killed at the deadline however fast it is.
pub fn run_with_deadline(cmd: &mut Command, deadline: Duration) -> Result<Output, String> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot wait for {program}: {e}"));
            }
        }
        if started.elapsed() >= deadline {
            // Killed: its pipes close, the two readers end by themselves.
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{program} timed out after {}s", deadline.as_secs()));
        }
        std::thread::sleep(POLL);
    };
    let read = |handle: Option<std::thread::JoinHandle<Vec<u8>>>| handle.and_then(|h| h.join().ok()).unwrap_or_default();
    Ok(Output { status, stdout: read(stdout), stderr: read(stderr) })
}

/// Read a child's pipe to its end on a thread of its own, so the child
/// never waits on a full pipe. What a failed read left is what it returns.
fn drain<R: std::io::Read + Send + 'static>(pipe: Option<R>) -> Option<std::thread::JoinHandle<Vec<u8>>> {
    let mut pipe = pipe?;
    Some(std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = pipe.read_to_end(&mut out);
        out
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_that_overruns_its_deadline_is_killed_and_says_so() {
        let started = Instant::now();
        let err = run_with_deadline(
            Command::new("/bin/sleep").arg("5"),
            Duration::from_secs(1),
        )
        .expect_err("sleep 5 must not finish within a second");
        assert!(err.contains("timed out"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(4), "the child was reaped at once");
    }
    /// 300 KB on stdout and as much on stderr: more than a pipe holds. The
    /// child ends at once and all of it is read (11/11/2026: `attn inflight`
    /// grew past 64 KB and timed out at every read).
    #[test]
    fn a_child_that_prints_more_than_a_pipe_holds_ends_and_is_read_whole() {
        let started = Instant::now();
        let out = run_with_deadline(
            Command::new("/bin/sh").args(["-c", "head -c 300000 /dev/zero; head -c 300000 /dev/zero >&2; exit 3"]),
            Duration::from_secs(10),
        )
        .expect("a fast child with a large output must not time out");
        assert_eq!(out.stdout.len(), 300_000);
        assert_eq!(out.stderr.len(), 300_000);
        assert_eq!(out.status.code(), Some(3), "its own exit status");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

}
