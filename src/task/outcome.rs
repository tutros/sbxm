//! `outcome.json` in a task's folder (decision 321(2), issue 612): how the last `task run`,
//! `task resume`, `task fix` or `task review` of the task ended, written as the command ends, by
//! hand or as the loop's child. `task loop` reads it, and the task record, to decide what to do
//! next: the child's exit code only says ok or not, and no error type or output text crosses the
//! process boundary. The matching that finds a usage limit or a lapsed sign-in is done here, by
//! the command that writes the file.
//!
//! The file is removed when the command names its task and written last, atomically (a temp file
//! and a rename), so one left from an earlier run is never read as this run's. A command refused
//! before its task has a folder writes none: nothing is created for it.

use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::finish::check_own_folder;
use super::{lock, pipeline, queue, record};
use crate::harness::Harness;

/// The file's name in the task's folder.
pub const FILE: &str = "outcome.json";

/// How a task command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Ended without an error: for `task run` and `task resume`, the task is ready.
    Ready,
    /// Any other failure.
    Failed,
    /// A provider's usage limit stopped it.
    UsageLimit,
    /// `sbx`'s sign-in, or a harness's, has lapsed.
    SignedOut,
    /// `sbx` didn't answer, or a sandbox couldn't run the command.
    Environment,
    /// A secret the task needs isn't set.
    MissingSecret,
    /// A stop request ended its wait for a slot: nothing was created.
    Stopped,
    /// A live command holds the task's lock: nothing was written.
    Held,
    /// Refused before the task had a record.
    Refused,
}

impl Kind {
    /// The word `outcome.json` uses.
    pub fn name(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::UsageLimit => "usage_limit",
            Self::SignedOut => "signed_out",
            Self::Environment => "environment",
            Self::MissingSecret => "missing_secret",
            Self::Stopped => "stopped",
            Self::Held => "held",
            Self::Refused => "refused",
        }
    }
}

/// `outcome.json`: `{"end": "<kind>", "provider": "<name or null>", "detail": "<text>"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub end: Kind,
    /// For a usage limit, the provider whose limit it was (`anthropic`, `openai`, `google`), when
    /// the failure names its harness; for a lapsed sign-in, `sbx` or the harness's provider.
    pub provider: Option<String>,
    /// The error as the command printed it; for `sbx`'s lapsed sign-in, `sbx`'s own words.
    pub detail: String,
}

impl Outcome {
    pub fn new(end: Kind, provider: Option<String>, detail: impl Into<String>) -> Self {
        Self {
            end,
            provider,
            detail: detail.into(),
        }
    }

    /// How a command whose result is `result` ended; `output` is what it printed, where a
    /// harness's failure line says a usage limit or a lapsed sign-in.
    pub fn of(result: &Result<()>, output: &str) -> Self {
        let Err(e) = result else {
            return Self::new(Kind::Ready, None, "");
        };
        let detail = format!("{e:#}");
        if e.is::<crate::slot::Stopped>() {
            return Self::new(Kind::Stopped, None, detail);
        }
        if e.is::<lock::InUse>() {
            return Self::new(Kind::Held, None, detail);
        }
        if e.is::<pipeline::MissingSecret>() {
            return Self::new(Kind::MissingSecret, None, detail);
        }
        if let Some(stop) = e.downcast_ref::<pipeline::NotSignedIn>() {
            return Self::new(Kind::SignedOut, Some(SBX.to_owned()), stop.said());
        }
        if says_harness_signed_out(e, output) {
            let provider = provider_of(&detail, output, pipeline::says_harness_signed_out);
            return Self::new(Kind::SignedOut, provider, detail);
        }
        if queue::failure_says_usage_limit(&detail, output) {
            let provider = provider_of(&detail, output, queue::is_usage_limit);
            return Self::new(Kind::UsageLimit, provider, detail);
        }
        if e.is::<pipeline::SbxUnavailable>() || e.is::<crate::headless::ExecUnavailable>() {
            return Self::new(Kind::Environment, None, detail);
        }
        Self::new(Kind::Failed, None, detail)
    }
}

/// The provider named for `sbx`'s own lapsed sign-in.
pub const SBX: &str = "sbx";

/// Whether a failed command stopped on a harness's lapsed sign-in (issue 445): said in its error,
/// or in a line of its output that reports a failure.
fn says_harness_signed_out(e: &anyhow::Error, output: &str) -> bool {
    e.downcast_ref::<pipeline::HarnessSignedOut>().is_some()
        || pipeline::says_harness_signed_out(&format!("{e:#}"))
        || output.lines().any(|line| {
            line.to_lowercase().contains("failed") && pipeline::says_harness_signed_out(line)
        })
}

/// The provider of the harness named in the text that says it (`says`): the error, else the
/// output's failure line that says it.
fn provider_of(error: &str, output: &str, says: fn(&str) -> bool) -> Option<String> {
    let lines = std::iter::once(error).filter(|e| says(e)).chain(
        output
            .lines()
            .filter(|line| line.to_lowercase().contains("failed") && says(line)),
    );
    lines
        .filter_map(harness_in)
        .next()
        .map(|h| h.provider_secret().to_owned())
}

/// The first harness named as a word of `text`.
fn harness_in(text: &str) -> Option<Harness> {
    let text = text.to_lowercase();
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .find_map(|word| {
            [
                Harness::Claude,
                Harness::Codex,
                Harness::Pi,
                Harness::Antigravity,
            ]
            .into_iter()
            .find(|h| h.as_str() == word)
        })
}

/// `outcome.json`'s path in the task folder `dir`.
pub fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// Removes the outcome an earlier run left in `dir`, if any.
pub fn remove(dir: &Path) -> Result<()> {
    remove_file(&path(dir))
}

/// Writes `outcome` into the existing task folder `dir`, through a temp file and a rename.
pub fn write(dir: &Path, outcome: &Outcome) -> Result<()> {
    write_file(&path(dir), outcome)
}

/// Writes `outcome` as the file `path`, through a temp file next to it and a rename.
pub fn write_file(path: &Path, outcome: &Outcome) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut text = serde_json::to_string_pretty(outcome)?;
    text.push('\n');
    fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("cannot write {}", path.display()))
}

/// What `outcome.json` in a task folder says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    /// No file: the command never wrote one (killed, crashed, or refused before a folder).
    Missing,
    Outcome(Outcome),
    /// A file that doesn't parse, or names a kind not known here: its text, and why.
    Unreadable {
        text: String,
        why: String,
    },
}

/// Reads `outcome.json` in the task folder `dir`.
pub fn read(dir: &Path) -> Read {
    read_file(&path(dir))
}

/// Reads the outcome file `path`.
pub fn read_file(path: &Path) -> Read {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Read::Missing,
        Err(e) => {
            return Read::Unreadable {
                text: String::new(),
                why: format!("cannot read {}: {e}", path.display()),
            };
        }
    };
    match serde_json::from_str(&text) {
        Ok(outcome) => Read::Outcome(outcome),
        Err(e) => Read::Unreadable {
            text,
            why: format!("{} doesn't parse: {e}", path.display()),
        },
    }
}

/// Removes the outcome file `path`, if any.
pub fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("cannot remove {}", path.display())),
    }
}

/// Where a running command's outcome goes, once it names its task.
#[derive(Default)]
pub struct Recorder {
    /// The base dir and the task's folder under it.
    at: RefCell<Option<(PathBuf, PathBuf)>>,
}

impl Recorder {
    /// The command works on task `id` under `base_dir`: an outcome left in its folder by an
    /// earlier run is removed now, and this run's is written there as it ends. A folder that isn't
    /// the task's own (a link, or one resolving elsewhere) gets none: the command refuses it.
    pub fn at(&self, base_dir: &Path, id: &str) -> Result<()> {
        if !record::is_valid_id(id) {
            return Ok(());
        }
        let dir = record::task_dir(base_dir, id);
        if dir.symlink_metadata().is_ok() {
            if check_own_folder(base_dir, &dir).is_err() {
                return Ok(());
            }
            if dir.is_dir() {
                remove(&dir)?;
            }
        }
        *self.at.borrow_mut() = Some((base_dir.to_owned(), dir));
        Ok(())
    }

    /// [`Self::at`] under the base dir of the config in `config_dir`. A config that can't be read
    /// names no folder: the command reports it itself.
    pub fn at_config(&self, config_dir: &Path, id: &str) -> Result<()> {
        match crate::config::GlobalConfig::load(config_dir) {
            Ok(global) => self.at(&global.base_dir, id),
            Err(_) => Ok(()),
        }
    }
}

/// Runs a task command, `step`, and writes its outcome as it ends into the folder it named with
/// [`Recorder::at`], if that folder exists then; else, for a child of `task loop`, into the file
/// the loop named in [`LOOP_FILE_ENV`]. A command whose task has no record by then is `refused`,
/// unless its stop is one of the machine's (`environment`, `stopped`, ...). A failure to write the
/// outcome is a warning: the command's own result stands.
pub fn recorded(
    out: &mut dyn Write,
    warn: &mut dyn Write,
    step: impl FnOnce(&Recorder, &mut dyn Write, &mut dyn Write) -> Result<()>,
) -> Result<()> {
    let elsewhere = std::env::var_os(LOOP_FILE_ENV)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty());
    recorded_with(elsewhere, out, warn, step)
}

/// [`recorded`], with `elsewhere` the file for an outcome whose task has no folder.
pub fn recorded_with(
    elsewhere: Option<PathBuf>,
    out: &mut dyn Write,
    warn: &mut dyn Write,
    step: impl FnOnce(&Recorder, &mut dyn Write, &mut dyn Write) -> Result<()>,
) -> Result<()> {
    let recorder = Recorder::default();
    let mut out = Capture::new(out);
    let mut warn = Capture::new(warn);
    let result = step(&recorder, &mut out, &mut warn);
    let own = recorder
        .at
        .into_inner()
        .filter(|(base_dir, dir)| dir.is_dir() && check_own_folder(base_dir, dir).is_ok())
        .map(|(_, dir)| dir);
    let file = match (&own, elsewhere) {
        (Some(dir), _) => path(dir),
        (None, Some(file)) => file,
        (None, None) => return result,
    };
    let mut text = out.text;
    text.extend(warn.text);
    let mut outcome = Outcome::of(&result, &String::from_utf8_lossy(&text));
    let has_record = own.is_some_and(|dir| dir.join("task.json").is_file());
    if outcome.end == Kind::Failed && !has_record {
        outcome.end = Kind::Refused;
    }
    if let Err(e) = write_file(&file, &outcome) {
        let _ = writeln!(
            warn.inner,
            "warning: {e:#}; the task's outcome isn't recorded"
        );
    }
    result
}

/// The environment variable naming, to a child of `task loop`, the file for its outcome when its
/// task has no folder: a command refused before it made one (a missing secret, `sbx` signed out
/// or not answering, a stop during its slot wait) creates none, and the loop must still read how
/// it ended.
pub const LOOP_FILE_ENV: &str = "SBXM_LOOP_OUTCOME";

/// The file `task loop` names in [`LOOP_FILE_ENV`] for task `id`'s child.
pub fn loop_file(base_dir: &Path, id: &str) -> PathBuf {
    base_dir
        .join(".sbxm")
        .join("loop-outcomes")
        .join(format!("{id}.json"))
}

/// Passes output on and keeps a copy, for the failure lines [`Outcome::of`] reads.
struct Capture<'a> {
    inner: &'a mut dyn Write,
    text: Vec<u8>,
}

impl<'a> Capture<'a> {
    fn new(inner: &'a mut dyn Write) -> Self {
        Self {
            inner,
            text: Vec::new(),
        }
    }
}

impl Write for Capture<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.text.extend_from_slice(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    fn of(e: anyhow::Error, output: &str) -> Outcome {
        Outcome::of(&Err(e), output)
    }

    #[test]
    fn ok_is_ready() {
        assert_eq!(Outcome::of(&Ok(()), "").end, Kind::Ready);
    }

    #[test]
    fn a_plain_error_is_failed_with_its_text() {
        let o = of(anyhow!("inner").context("outer"), "");
        assert_eq!(o, Outcome::new(Kind::Failed, None, "outer: inner"));
    }

    #[test]
    fn a_usage_limit_in_a_failure_line_names_its_provider() {
        let o = of(
            anyhow!("the worker failed"),
            "issue-3: worker codex failed: You've hit your usage limit\n",
        );
        assert_eq!(o.end, Kind::UsageLimit);
        assert_eq!(o.provider.as_deref(), Some("openai"));
    }

    #[test]
    fn a_usage_limit_with_no_harness_named_has_no_provider() {
        let o = of(anyhow!("usage limit reached"), "");
        assert_eq!(o.end, Kind::UsageLimit);
        assert_eq!(o.provider, None);
    }

    #[test]
    fn words_about_a_usage_limit_in_other_lines_are_not_one() {
        let o = of(anyhow!("boom"), "the issue: wait out the usage limit\n");
        assert_eq!(o.end, Kind::Failed);
    }

    #[test]
    fn sbx_signed_out_keeps_sbxs_words() {
        let stop = pipeline::NotSignedIn::new("Not authenticated to Docker")
            .then(vec!["sbxm task run --issue 3".into()]);
        let o = of(anyhow::Error::new(stop).context("starting"), "");
        assert_eq!(
            o,
            Outcome::new(
                Kind::SignedOut,
                Some("sbx".into()),
                "Not authenticated to Docker"
            )
        );
    }

    #[test]
    fn the_machines_stops_have_their_kinds() {
        let cases: Vec<(anyhow::Error, Kind)> = vec![
            (crate::slot::Stopped.into(), Kind::Stopped),
            (
                pipeline::SbxUnavailable::new("sbx is gone").into(),
                Kind::Environment,
            ),
            (
                crate::headless::ExecUnavailable("no exec".into()).into(),
                Kind::Environment,
            ),
            (
                pipeline::MissingSecret::new("anthropic").into(),
                Kind::MissingSecret,
            ),
        ];
        for (e, kind) in cases {
            assert_eq!(of(e, "").end, kind);
        }
    }

    #[test]
    fn harness_names_are_whole_words() {
        assert_eq!(harness_in("worker pi failed"), Some(Harness::Pi));
        assert_eq!(harness_in("pipeline failed"), None);
    }

    #[test]
    fn an_unknown_kind_or_bad_json_is_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        for text in [
            r#"{"end": "exploded", "provider": null, "detail": ""}"#,
            "not json",
        ] {
            fs::write(path(dir.path()), text).unwrap();
            assert!(
                matches!(read(dir.path()), Read::Unreadable { text: t, .. } if t == text),
                "{text}"
            );
        }
    }

    #[test]
    fn written_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(dir.path()), Read::Missing);
        let o = Outcome::new(Kind::UsageLimit, Some("anthropic".into()), "limit");
        write(dir.path(), &o).unwrap();
        assert_eq!(read(dir.path()), Read::Outcome(o));
        let text = fs::read_to_string(path(dir.path())).unwrap();
        assert!(text.contains(r#""end": "usage_limit""#), "{text}");
        assert!(!dir.path().join("outcome.json.tmp").exists());
    }

    #[test]
    fn recorded_removes_a_stale_outcome_and_writes_none_without_a_folder() {
        let base = tempfile::tempdir().unwrap();
        let dir = record::task_dir(base.path(), "issue-3");
        fs::create_dir_all(&dir).unwrap();
        write(&dir, &Outcome::new(Kind::Ready, None, "")).unwrap();
        let gone = record::task_dir(base.path(), "issue-4");
        let r = recorded(&mut Vec::new(), &mut Vec::new(), |rec, _, _| {
            rec.at(base.path(), "issue-3")?;
            assert_eq!(read(&dir), Read::Missing, "removed as the task is named");
            rec.at(base.path(), "issue-4")?;
            Err(anyhow!("refused"))
        });
        assert!(r.is_err());
        assert!(!gone.exists(), "nothing created");
    }

    #[test]
    fn with_no_task_folder_the_outcome_goes_where_the_loop_said() {
        let base = tempfile::tempdir().unwrap();
        let elsewhere = base.path().join("issue-3.json");
        let r = recorded_with(
            Some(elsewhere.clone()),
            &mut Vec::new(),
            &mut Vec::new(),
            |rec, _, _| {
                rec.at(base.path(), "issue-3")?;
                Err(pipeline::MissingSecret::new("no anthropic secret").into())
            },
        );
        assert!(r.is_err());
        assert!(!record::task_dir(base.path(), "issue-3").exists());
        assert_eq!(
            read_file(&elsewhere),
            Read::Outcome(Outcome::new(
                Kind::MissingSecret,
                None,
                "no anthropic secret"
            ))
        );
        // Refused before any folder: refused.
        recorded_with(
            Some(elsewhere.clone()),
            &mut Vec::new(),
            &mut Vec::new(),
            |_, _, _| Err(anyhow!("no such issue")),
        )
        .unwrap_err();
        assert!(matches!(read_file(&elsewhere), Read::Outcome(o) if o.end == Kind::Refused));
    }

    #[test]
    fn a_failure_with_no_record_is_refused() {
        let base = tempfile::tempdir().unwrap();
        let dir = record::task_dir(base.path(), "issue-3");
        fs::create_dir_all(&dir).unwrap();
        let r = recorded(&mut Vec::new(), &mut Vec::new(), |rec, _, _| {
            rec.at(base.path(), "issue-3")?;
            Err(anyhow!("no such issue"))
        });
        assert!(r.is_err());
        assert_eq!(
            read(&dir),
            Read::Outcome(Outcome::new(Kind::Refused, None, "no such issue"))
        );
    }

    #[test]
    fn the_output_is_read_for_a_usage_limit() {
        let base = tempfile::tempdir().unwrap();
        let dir = record::task_dir(base.path(), "issue-3");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("task.json"), "{}").unwrap();
        let mut out = Vec::new();
        let _ = recorded(&mut out, &mut Vec::new(), |rec, out, _| {
            rec.at(base.path(), "issue-3")?;
            writeln!(out, "issue-3: worker claude failed: usage limit reached")?;
            Err(anyhow!("the worker failed"))
        });
        assert!(String::from_utf8(out).unwrap().contains("usage limit"));
        let Read::Outcome(o) = read(&dir) else {
            panic!()
        };
        assert_eq!(
            (o.end, o.provider.as_deref()),
            (Kind::UsageLimit, Some("anthropic"))
        );
    }
}
