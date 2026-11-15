//! The screen is best-effort (issue 667, answer 2): a write to stdout or stderr that fails (a
//! broken pipe, a closed console) is dropped and never fails the command. A child of a killed
//! `task loop`, or a command whose reader went away, finishes its work and ends with its true
//! outcome. Records (`task.json`, `outcome.json`, `run.log`, the loop's logs) are files written
//! with their own errors, never through this.
//!
//! A child `sbxm` of `task loop` has no screen: the loop names its log in [`LOG_ENV`], and what
//! it prints goes there instead, one line at a time with every value that looks like a secret
//! masked as `run.log` masks it (issue 667, review M-1). The log is the screen's stand-in, so a
//! write to it is best-effort too.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use crate::task::runlog::redact;

/// The environment variable naming, to a child of `task loop`, the log its screen goes to.
pub const LOG_ENV: &str = "SBXM_LOOP_CHILD_LOG";

/// This process's screen as a masked log, once [`log_to`] set it.
static LOG: OnceLock<Mutex<MaskedLog<File>>> = OnceLock::new();

/// Which of the screen's two streams a write is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Out,
    Err,
}

/// What a process prints, written to `file` a whole line at a time, masked: a value split across
/// two writes is masked once its line is whole. Each stream keeps its own partial line.
pub struct MaskedLog<W: Write> {
    file: W,
    out: Vec<u8>,
    err: Vec<u8>,
}

impl<W: Write> MaskedLog<W> {
    pub fn new(file: W) -> Self {
        Self {
            file,
            out: Vec::new(),
            err: Vec::new(),
        }
    }

    /// Takes `buf` on `stream`, writing out each line it completes.
    pub fn push(&mut self, stream: Stream, buf: &[u8]) {
        let partial = match stream {
            Stream::Out => &mut self.out,
            Stream::Err => &mut self.err,
        };
        partial.extend_from_slice(buf);
        let Some(end) = partial.iter().rposition(|&b| b == b'\n') else {
            return;
        };
        let lines: Vec<u8> = partial.drain(..=end).collect();
        let _ = self.file.write_all(masked(&lines).as_bytes());
    }

    /// Writes out both streams' partial lines, each ended: the process is ending.
    pub fn finish(&mut self) {
        for partial in [std::mem::take(&mut self.out), std::mem::take(&mut self.err)] {
            if !partial.is_empty() {
                let _ = self
                    .file
                    .write_all(format!("{}\n", masked(&partial)).as_bytes());
            }
        }
        let _ = self.file.flush();
    }

    pub fn into_inner(mut self) -> W {
        self.finish();
        self.file
    }
}

fn masked(bytes: &[u8]) -> String {
    redact(&String::from_utf8_lossy(bytes))
}

/// Sends this process's screen to the log at `path`, masked ([`MaskedLog`]), for the rest of
/// its life: a child of `task loop` that the loop named its log to.
pub fn log_to(path: &Path) -> io::Result<()> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let _ = LOG.set(Mutex::new(MaskedLog::new(file)));
    Ok(())
}

/// Writes out the log's partial lines, if the screen is a log: call before the process exits.
pub fn finish() {
    if let Some(log) = LOG.get() {
        log.lock().unwrap_or_else(|e| e.into_inner()).finish();
    }
}

/// A writer to the screen whose failures are dropped; the process's own stdout and stderr go to
/// its log instead once there is one ([`log_to`]).
pub struct Screen<W: Write> {
    inner: W,
    stream: Option<Stream>,
}

impl<W: Write> Screen<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            stream: None,
        }
    }
}

/// The process's stdout, best-effort.
pub fn stdout() -> Screen<io::Stdout> {
    Screen {
        inner: io::stdout(),
        stream: Some(Stream::Out),
    }
}

/// The process's stderr, best-effort.
pub fn stderr() -> Screen<io::Stderr> {
    Screen {
        inner: io::stderr(),
        stream: Some(Stream::Err),
    }
}

/// `text` on stdout, best-effort: `print!` without its panic.
pub fn print(text: &str) {
    let _ = stdout().write_all(text.as_bytes());
}

/// `text` and a newline on stdout, best-effort: `println!` without its panic.
pub fn println(text: &str) {
    print(&format!("{text}\n"));
}

/// `text` on stderr, best-effort: `eprint!` without its panic.
pub fn eprint(text: &str) {
    let _ = stderr().write_all(text.as_bytes());
}

/// `text` and a newline on stderr, best-effort: `eprintln!` without its panic.
pub fn eprintln(text: &str) {
    eprint(&format!("{text}\n"));
}

impl<W: Write> Write for Screen<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let (Some(stream), Some(log)) = (self.stream, LOG.get()) {
            log.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(stream, buf);
            return Ok(buf.len());
        }
        match self.inner.write(buf) {
            Ok(n) if n > 0 || buf.is_empty() => Ok(n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => Err(e),
            // Gone, or taking nothing: the text is dropped.
            Ok(_) | Err(_) => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = self.inner.flush();
        Ok(())
    }
}
