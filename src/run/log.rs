//! `run.log` in a run's folder, `<base>/.sbxm/runs/<id>/` (decision 170, part 3): a copy of what
//! `sbxm run` prints, with the same header and line format as a task's log (`task::runlog`, part
//! 1), the stage being the part of the run a line was printed in. The folder only exists once the
//! run is reserved, so the lines printed before (preflight warnings) are kept and written then.
//! Nothing is logged that the screen did not show, and values that look like secrets are masked.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::run::results::format_timestamp;
use crate::task::runlog::{LineSink, redact};

/// Before the run folder exists: checking the config, the secrets and the profiles.
pub const PREFLIGHT: &str = "preflight";
/// Running the contestants.
pub const PAIRS: &str = "pairs";
pub const JUDGE: &str = "judge";
pub const COSINE: &str = "cosine";
pub const JEV: &str = "jev";
/// The rankings, marking the run completed and where its results are.
pub const RESULTS: &str = "results";

pub struct RunFolderLog {
    /// `None` for a log that writes nowhere (its header could not be made).
    header: Option<String>,
    clock: Box<dyn Fn() -> u64 + Send>,
    /// Told once, not once per line, when the log can't be appended to.
    warn: Box<dyn FnMut(&str) + Send>,
    stage: &'static str,
    /// `run.log`, once the run folder exists.
    path: Option<PathBuf>,
    /// The stamped lines printed before the run folder existed.
    pending: Vec<String>,
    warned: bool,
}

impl RunFolderLog {
    pub fn new(
        header: String,
        clock: Box<dyn Fn() -> u64 + Send>,
        warn: Box<dyn FnMut(&str) + Send>,
    ) -> Self {
        Self {
            header: Some(header),
            clock,
            warn,
            stage: PREFLIGHT,
            path: None,
            pending: Vec::new(),
            warned: false,
        }
    }

    /// A log that writes nowhere, for a run whose header could not be made (an unreadable
    /// executable), which says so itself.
    pub fn disabled() -> Self {
        Self {
            header: None,
            ..Self::new(String::new(), Box::new(|| 0), Box::new(|_| {}))
        }
    }

    /// The part of the run the next lines are printed in.
    pub fn set_stage(&mut self, stage: &'static str) {
        self.stage = stage;
    }

    /// Starts the log in the run folder `dir`, just reserved: the header, then every line
    /// printed so far.
    pub fn attach(&mut self, dir: &Path) {
        let Some(header) = &self.header else {
            return;
        };
        let mut text = format!("{header}\n");
        self.path = Some(dir.join("run.log"));
        for line in std::mem::take(&mut self.pending) {
            text.push_str(&line);
        }
        self.append(&text);
    }

    /// Every line of `text` (an error's chain, say), each on its own stamped line.
    pub fn lines(&mut self, text: &str) {
        for line in text.lines() {
            self.line(line);
        }
    }

    fn append(&mut self, text: &str) {
        let Some(path) = &self.path else {
            return;
        };
        let written = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| file.write_all(text.as_bytes()));
        if let Err(err) = written
            && !self.warned
        {
            self.warned = true;
            let message = format!("warning: cannot write {}: {err}", path.display());
            (self.warn)(&message);
        }
    }
}

impl LineSink for RunFolderLog {
    /// A log that can't be written is skipped: the screen already has the line, and the run must
    /// not fail for it.
    fn line(&mut self, text: &str) {
        let line = format!(
            "{} [{}] {}\n",
            format_timestamp((self.clock)()),
            self.stage,
            redact(text)
        );
        if self.path.is_some() {
            self.append(&line);
        } else if self.header.is_some() {
            self.pending.push(line);
        }
    }
}
