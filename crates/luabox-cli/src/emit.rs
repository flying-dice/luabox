//! Writing to the user's terminal without dying when the reader hangs up.
//!
//! `println!` and its siblings `panic!` on a failed write — that is not a
//! configuration, it is what `std::io::stdio::print_to` does. A pipeline as
//! ordinary as `luabox check | head -2` closes the read end as soon as `head`
//! has its two lines, so every subsequent write to stdout fails with `EPIPE`,
//! and any report bigger than the pipe buffer (64 KiB on Linux) is guaranteed
//! to still be mid-write when that happens. The result was a raw Rust panic
//! plus a backtrace on stderr and exit status 101 — and, in the shipped
//! release profile (`panic = "abort"`), `SIGABRT`, i.e. status 134. A CI job
//! running `set -o pipefail` with a routine `| head` or `| grep -q` went red
//! for it, in all five `--format`s.
//!
//! ## The rule
//!
//! A closed pipe is not an error. It means the reader got what it wanted and
//! left, so luabox stops writing and exits **0**, quietly — ripgrep's
//! convention, and the one `head` users already expect. That applies to
//! stderr as well as stdout: `luabox check 2>&1 | head` puts both streams on
//! the same dead pipe, and a diagnostics summary has no more claim on a
//! departed reader than the report does.
//!
//! Any *other* write failure (a full disk on `luabox schema > out.json`, say)
//! is a real failure and exits 1, with a one-line explanation on stderr —
//! best-effort, since the stream we would explain it on may be the broken
//! one. Nothing here panics; that is the point of the module.
//!
//! ## The shape
//!
//! [`classify`] is the whole decision, and it is a pure function of an
//! `io::Result` — so the mapping is unit tested directly, including against a
//! writer that fails, without any test having to survive a `process::exit`.
//! [`to_stdout`] and [`to_stderr`] are the thin edge that acts on it, and
//! they are the only places in the crate that exit the process this way. The
//! [`outln!`], [`out!`], [`errln!`] and [`err!`] macros mirror the `std`
//! macros they replace, so a call site reads the same as before and no
//! command signature had to change to thread a `Result` back out.

use std::io::{self, ErrorKind, Write};

/// What a user-facing write leaves the stream in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    /// The bytes landed. Carry on.
    Continue,
    /// The reader closed the pipe (`EPIPE`). There is nobody left to write
    /// to, and that is a success — see the module docs.
    ReaderGone,
}

/// Map the outcome of a user-facing write onto what the process should do:
/// a broken pipe becomes [`Flow::ReaderGone`] rather than an error, and
/// everything else is passed through unchanged.
///
/// Split out from the writing so the decision — the only part with any
/// judgement in it — is testable without a process exit in the way.
fn classify(result: io::Result<()>) -> io::Result<Flow> {
    match result {
        Ok(()) => Ok(Flow::Continue),
        Err(error) if error.kind() == ErrorKind::BrokenPipe => Ok(Flow::ReaderGone),
        Err(error) => Err(error),
    }
}

/// Write `text` to `stream` and classify the outcome ([`classify`]).
///
/// The explicit `flush` matters: stdout is line-buffered, so without it a
/// final fragment could sit in the buffer and fail during the implicit flush
/// at exit — where nothing is left to handle it.
fn write_text(stream: &mut impl Write, text: &str) -> io::Result<Flow> {
    classify(
        stream
            .write_all(text.as_bytes())
            .and_then(|()| stream.flush()),
    )
}

/// Act on a write outcome: carry on, leave quietly, or fail loudly.
///
/// One of the crate's two `process::exit` sites (the other is its twin in
/// [`to_stderr`]) — deliberately trivial, so that everything worth testing
/// lives in [`classify`] instead.
fn act(outcome: io::Result<Flow>, stream: &str) {
    match outcome {
        Ok(Flow::Continue) => {}
        Ok(Flow::ReaderGone) => std::process::exit(0),
        Err(error) => {
            // The stream we would normally explain this on may be the one
            // that just failed, so this is best-effort by construction — and
            // `writeln!` returns the failure rather than panicking on it,
            // unlike the `eprintln!` this replaces.
            let _ = writeln!(io::stderr(), "Error: cannot write to {stream}: {error}");
            std::process::exit(1);
        }
    }
}

/// Write `text` to stdout, verbatim. See the module docs for what happens
/// when the reader has gone away.
pub(crate) fn to_stdout(text: &str) {
    let mut stream = io::stdout().lock();
    act(write_text(&mut stream, text), "stdout");
}

/// Write `text` to stderr, verbatim. See the module docs for what happens
/// when the reader has gone away.
pub(crate) fn to_stderr(text: &str) {
    let mut stream = io::stderr().lock();
    act(write_text(&mut stream, text), "stderr");
}

/// `println!` that survives a closed pipe — see the module docs.
macro_rules! outln {
    () => { $crate::emit::to_stdout("\n") };
    ($($arg:tt)*) => { $crate::emit::to_stdout(&format!("{}\n", format_args!($($arg)*))) };
}

/// `print!` that survives a closed pipe — see the module docs.
macro_rules! out {
    ($($arg:tt)*) => { $crate::emit::to_stdout(&format!("{}", format_args!($($arg)*))) };
}

/// `eprintln!` that survives a closed pipe — see the module docs.
macro_rules! errln {
    ($($arg:tt)*) => { $crate::emit::to_stderr(&format!("{}\n", format_args!($($arg)*))) };
}

/// `eprint!` that survives a closed pipe — see the module docs.
macro_rules! err {
    ($($arg:tt)*) => { $crate::emit::to_stderr(&format!("{}", format_args!($($arg)*))) };
}

pub(crate) use {err, errln, out, outln};

#[cfg(test)]
mod tests {
    // test code — panics document assumptions
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{Flow, classify, write_text};
    use std::io::{self, ErrorKind, Write};

    /// A sink that fails every write with a caller-chosen error — the only
    /// way to drive the `EPIPE` path deterministically, since a real closed
    /// pipe cannot be arranged inside a unit test without a child process.
    struct Failing(ErrorKind);

    impl Write for Failing {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(self.0, "stub"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::new(self.0, "stub"))
        }
    }

    #[test]
    fn a_write_that_lands_says_carry_on() {
        assert_eq!(classify(Ok(())).expect("not a failure"), Flow::Continue);
    }

    #[test]
    fn a_broken_pipe_is_not_an_error_but_a_departed_reader() {
        // The whole finding: `luabox check | head` must not be a failure.
        let outcome = classify(Err(io::Error::new(ErrorKind::BrokenPipe, "head exited")));
        assert_eq!(outcome.expect("EPIPE is not a failure"), Flow::ReaderGone);
    }

    #[test]
    fn any_other_write_failure_stays_an_error() {
        // A full disk on `luabox schema > out.json` is a real failure and
        // must not be mistaken for a reader that simply left.
        for kind in [
            ErrorKind::StorageFull,
            ErrorKind::PermissionDenied,
            ErrorKind::Interrupted,
        ] {
            let outcome = classify(Err(io::Error::new(kind, "stub")));
            assert_eq!(
                outcome.expect_err("must stay an error").kind(),
                kind,
                "{kind:?} must be reported, not swallowed"
            );
        }
    }

    #[test]
    fn writing_to_a_healthy_stream_delivers_the_bytes_verbatim() {
        let mut sink = Vec::new();
        assert_eq!(
            write_text(&mut sink, "watch: ok\n").expect("wrote"),
            Flow::Continue
        );
        assert_eq!(sink, b"watch: ok\n");
    }

    #[test]
    fn writing_to_a_closed_pipe_reports_a_departed_reader_rather_than_failing() {
        let mut sink = Failing(ErrorKind::BrokenPipe);
        assert_eq!(
            write_text(&mut sink, "a very long report\n").expect("EPIPE is not an error"),
            Flow::ReaderGone
        );
    }

    #[test]
    fn writing_to_a_full_disk_propagates_the_failure() {
        let mut sink = Failing(ErrorKind::StorageFull);
        assert_eq!(
            write_text(&mut sink, "a very long report\n")
                .expect_err("a full disk is a real failure")
                .kind(),
            ErrorKind::StorageFull
        );
    }

    #[test]
    fn a_flush_failure_is_classified_like_a_write_failure() {
        // Stdout is line-buffered, so a fragment can fail at flush time
        // rather than at write time; both must reach the same verdict.
        struct FlushOnly;
        impl Write for FlushOnly {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::new(ErrorKind::BrokenPipe, "stub"))
            }
        }
        assert_eq!(
            write_text(&mut FlushOnly, "partial line").expect("EPIPE is not an error"),
            Flow::ReaderGone
        );
    }
}
