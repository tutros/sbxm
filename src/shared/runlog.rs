//! What a command's log needs on both sides (decision 170): masking values that look like
//! secrets, and a writer that hands each whole line it passes on to a log, a task's `run.log`
//! (`task::runlog`) or a run folder's (`run::log`).

use std::io::Write;
use std::sync::{Arc, LazyLock, Mutex};

use regex::Regex;

/// Values that look like an API key or token: `sk-…`, `ghp_…` (and the other `gh*_` kinds),
/// `github_pat_…` and Google `AIza…` keys.
static SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:sk-[A-Za-z0-9_-]{16,}|gh[pousr]_[A-Za-z0-9]{16,}|github_pat_[A-Za-z0-9_]{16,}|AIza[A-Za-z0-9_-]{16,})")
        .expect("the secret pattern is valid")
});

/// `text` with every value that looks like a secret replaced by `[redacted]`.
pub fn redact(text: &str) -> String {
    SECRET.replace_all(text, "[redacted]").into_owned()
}

/// Where a [`Tee`] sends each whole line: a task's `RunLog` (`task::runlog`), or a run folder's log
/// (`run::log::RunFolderLog`, decision 170, part 3).
pub trait LineSink {
    fn line(&mut self, text: &str);
}

/// A writer that passes everything to `inner` unchanged and hands each whole line to the log.
pub struct Tee<W: Write, L: LineSink> {
    inner: W,
    log: Arc<Mutex<L>>,
    partial: Vec<u8>,
}

impl<W: Write, L: LineSink> Tee<W, L> {
    pub fn new(inner: W, log: Arc<Mutex<L>>) -> Self {
        Self {
            inner,
            log,
            partial: Vec::new(),
        }
    }

    fn ship(&mut self, all: bool) {
        while let Some(end) = self.partial.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            self.send(&line[..end]);
        }
        if all && !self.partial.is_empty() {
            let rest = std::mem::take(&mut self.partial);
            self.send(&rest);
        }
    }

    fn send(&self, line: &[u8]) {
        let text = String::from_utf8_lossy(line);
        if let Ok(mut log) = self.log.lock() {
            log.line(text.trim_end_matches('\r'));
        }
    }
}

impl<W: Write, L: LineSink> Write for Tee<W, L> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.partial.extend_from_slice(&buf[..written]);
        self.ship(false);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write, L: LineSink> Drop for Tee<W, L> {
    fn drop(&mut self) {
        self.ship(true);
    }
}
