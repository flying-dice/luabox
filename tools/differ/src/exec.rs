//! Running a Lua program under a resolved interpreter, capturing its output
//! with a wall-clock timeout.
//!
//! Output is drained on dedicated threads so a program that writes more than a
//! pipe buffer's worth can't deadlock against our own `wait`, and the child is
//! killed if it outlives the timeout — a mis-lowered backward `goto` that
//! turns into an infinite loop must be *caught*, not hang the harness.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The captured result of one interpreter run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    /// The process exit code, or `None` if it was terminated by a signal.
    pub code: Option<i32>,
    /// True if we killed it for exceeding the timeout.
    pub timed_out: bool,
}

impl ExecResult {
    /// A run "failed" if it timed out or exited non-zero — the trigger for
    /// comparing error class on top of stdout and exit code.
    pub fn failed(&self) -> bool {
        self.timed_out || self.code != Some(0)
    }
}

/// Run `program script` (a bare interpreter invocation: no extra args) with
/// stdin closed, capturing stdout/stderr as lossy UTF-8, killing the child if
/// it runs longer than `timeout`.
pub fn run(program: &str, script: &Path, timeout: Duration) -> std::io::Result<ExecResult> {
    let mut child = Command::new(program)
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut out_pipe = child.stdout.take().expect("stdout piped");
    let mut err_pipe = child.stderr.take().expect("stderr piped");
    let out_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out_pipe.read_to_end(&mut buf);
        buf
    });
    let err_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err_pipe.read_to_end(&mut buf);
        buf
    });

    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            timed_out = true;
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(15));
    };

    let stdout = String::from_utf8_lossy(&out_handle.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err_handle.join().unwrap_or_default()).into_owned();

    Ok(ExecResult {
        stdout,
        stderr,
        code: status.code(),
        timed_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_reflects_exit_and_timeout() {
        let ok = ExecResult {
            stdout: String::new(),
            stderr: String::new(),
            code: Some(0),
            timed_out: false,
        };
        assert!(!ok.failed());
        let bad = ExecResult {
            code: Some(1),
            ..ok.clone()
        };
        assert!(bad.failed());
        let hung = ExecResult {
            timed_out: true,
            ..ok.clone()
        };
        assert!(hung.failed());
    }
}
