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
pub fn run_with_deadline(cmd: &mut Command, deadline: Duration) -> Result<Output, String> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot wait for {program}: {e}"));
            }
        }
        if started.elapsed() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{program} timed out after {}s", deadline.as_secs()));
        }
        std::thread::sleep(POLL);
    }
    child.wait_with_output().map_err(|e| format!("cannot read {program}'s output: {e}"))
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
}
