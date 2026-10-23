//! `sbxm task logs (--issue N | --pr N | --spec FILE) [--follow]` (decision 189, issue 88, part
//! 2 of 6 of decisions 170/171): prints a task's `run.log` (decision 170, part 1). Read-only:
//! never writes to the log itself, with or without `--follow` (decided point 3), and does not
//! use `task_writers` / `RunLog`, which would.

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};

use crate::config::GlobalConfig;
use crate::task::record;

/// Which task's log is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Issue(u32),
    Pr(u32),
    /// A spec task's file (decision 174(d)).
    Spec(PathBuf),
}

pub struct Options {
    pub target: Target,
    /// Keep printing new lines until Ctrl+C (decided point 2); never stops by itself.
    pub follow: bool,
}

/// What `--follow` needs from the environment, so tests don't depend on a real sleep or run
/// forever (the same pattern as `commands::status::Env`'s `--watch` loop).
pub struct Env {
    pub sleep: Box<dyn Fn(Duration)>,
    /// Test-only: stop after this many polls instead of running until Ctrl+C. `None` in
    /// [`Env::real`].
    pub iterations: Option<usize>,
}

impl Env {
    pub fn real() -> Self {
        Self {
            sleep: Box::new(std::thread::sleep),
            iterations: None,
        }
    }
}

/// How long `--follow` waits between polls of the log file.
const POLL: Duration = Duration::from_millis(500);

/// Prints the task's `run.log` and, with `--follow`, keeps polling for lines appended after it
/// started (decided point 4: a missing log is reported once and never waited for, `--follow` or
/// not).
pub fn run(config_dir: &Path, opts: &Options, env: &Env, out: &mut dyn Write) -> Result<()> {
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
    let id = match &opts.target {
        Target::Issue(n) => format!("issue-{n}"),
        Target::Pr(n) => format!("pr-{n}"),
        Target::Spec(path) => record::spec_id(path)?.0,
    };
    let dir = record::task_dir(&base_dir, &id);
    if !dir.join("task.json").is_file() {
        bail!("no task {id}; run `sbxm task status` to list the tasks");
    }
    let log = dir.join("run.log");
    // Issue 232: only a log that isn't there is "not there yet"; any other error names the file
    // and the OS's own text. A first read that fails counts as not opening it, so a folder named
    // `run.log` reads the same on Linux (open succeeds, read fails) as on Windows (open fails).
    let mut file = File::open(&log).map_err(|e| open_error(&id, &dir, &log, &e))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|e| open_error(&id, &dir, &log, &e))?;
    out.write_all(&buf)?;
    out.flush()?;
    if !opts.follow {
        return Ok(());
    }
    let mut polls = 0usize;
    loop {
        (env.sleep)(POLL);
        buf.clear();
        file.read_to_end(&mut buf)?;
        if !buf.is_empty() {
            out.write_all(&buf)?;
            out.flush()?;
        }
        polls += 1;
        if env.iterations.is_some_and(|n| polls >= n) {
            break;
        }
    }
    Ok(())
}

/// The error for a `run.log` that couldn't be opened (issue 232).
fn open_error(id: &str, dir: &Path, log: &Path, err: &std::io::Error) -> anyhow::Error {
    if err.kind() == ErrorKind::NotFound {
        anyhow::anyhow!("task {id} has no run.log yet; see {}", dir.display())
    } else {
        anyhow::anyhow!("cannot open {}: {err}", log.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Issue 232, the rule itself: every error other than not-found names the path and the OS
    /// error text.
    #[test]
    fn every_error_but_not_found_names_the_path_and_the_os_error() {
        let dir = Path::new("tasks").join("issue-41");
        let log = dir.join("run.log");
        for kind in [
            ErrorKind::PermissionDenied,
            ErrorKind::IsADirectory,
            ErrorKind::InvalidInput,
            ErrorKind::Other,
        ] {
            let err = std::io::Error::new(kind, "the os text");
            assert_eq!(
                open_error("issue-41", &dir, &log, &err).to_string(),
                format!("cannot open {}: the os text", log.display()),
                "{kind:?}"
            );
        }
        let not_found = std::io::Error::from(ErrorKind::NotFound);
        assert_eq!(
            open_error("issue-41", &dir, &log, &not_found).to_string(),
            format!("task issue-41 has no run.log yet; see {}", dir.display())
        );
    }
}
