//! The loop's improvement review (issue 715, decision 330(2) and (3)): at an idle wait, at most
//! once an hour ([`retro::CHECK_EVERY`]), the loop reads the review files on the default branch.
//! Each review a job of its own queued that has landed there since is one toast naming its
//! findings. Then, unless an improvement job is queued or running, or its PR is open, and when a
//! review is due ([`improvement::due`]), it writes the spec of the next area
//! (`<specs_dir>/improvement-<slug>-<date>.md`, never over a file) and queues it as `task queue
//! spec` does ([`task_queue::add`]). A failed read is one warning, and one line in the loop's own
//! log, and the check waits for the next hour; an `areas.md` that can't be used is one line a run,
//! and queues nothing. The loop goes on either way.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use super::{Loop, first_line};
use crate::commands::{task_facts, task_queue};
use crate::config::GlobalConfig;
use crate::notify::Notification;
use crate::task::improvement::{self, Area, Due, Listing, LoopLog};
use crate::task::jobs::Job;
use crate::task::{loop_log, queue, retro};

impl Loop<'_> {
    pub(super) fn check_improvement(
        &mut self,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        let now = self.now();
        if self
            .improvement_checked
            .is_some_and(|at| now.saturating_sub(at) < retro::CHECK_EVERY)
        {
            return Ok(());
        }
        self.improvement_checked = Some(now);
        let config = match GlobalConfig::load(self.config_dir) {
            Ok(config) => config,
            Err(e) => return self.improvement_failed("could not read the config", &e, warn),
        };
        let listing = match improvement::listing(self.github, &self.repo) {
            Ok(listing) => listing,
            Err(e) => return self.improvement_failed("could not read GitHub", &e, warn),
        };
        if let Some(specs_dir) = config.specs_dir.as_deref()
            && let Err(e) = self.toast_merged_reviews(&listing, specs_dir, out, warn)
        {
            return self.improvement_failed("could not read GitHub", &e, warn);
        }
        match improvement::in_flight(&self.base_dir, &self.repo, self.github) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(e) => return self.improvement_failed("could not read the queue", &e, warn),
        }
        let due = match improvement::due(self.github, &self.repo, &listing) {
            Ok(Some(due)) => due,
            Ok(None) => return Ok(()),
            Err(e) => return self.improvement_failed("could not read GitHub", &e, warn),
        };
        let area = match improvement::next_area(self.github, &self.repo, &listing) {
            Ok(Ok(area)) => area,
            Ok(Err(why)) => {
                let line = format!(
                    "the improvement review is due, but {}/{} {why}; no improvement review is \
                     queued",
                    improvement::DIR,
                    improvement::AREAS
                );
                return self.improvement_note_once(line, warn);
            }
            Err(e) => return self.improvement_failed("could not read GitHub", &e, warn),
        };
        let Some(specs_dir) = config.specs_dir.clone() else {
            let line = format!(
                "the improvement review of {} is due, but no specs_dir is set in {}, so no spec \
                 job can be queued; set specs_dir to queue it",
                area.slug,
                self.config_dir.join("config.toml").display()
            );
            return self.improvement_note_once(line, warn);
        };
        self.queue_improvement(&area, &due, &specs_dir, out, warn)
    }

    /// Writes the spec of `area`'s review in `specs_dir` and queues it, logging what it did. The
    /// spec file is removed again when the job can't be queued.
    fn queue_improvement(
        &mut self,
        area: &Area,
        due: &Due,
        specs_dir: &Path,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        let date = improvement::today(self.now());
        let name = improvement::spec_name(&area.slug, &date);
        let path = specs_dir.join(&name);
        if fs::symlink_metadata(&path).is_ok() {
            let line = format!(
                "the improvement review of {} is due, but {} already exists; no improvement review \
                 is queued over it",
                area.slug,
                path.display()
            );
            return self.improvement_note_once(line, warn);
        }
        let data = match self.run_data(due.since.as_deref()) {
            Ok(data) => data,
            Err(e) => return self.improvement_failed("could not read the run data", &e, warn),
        };
        let spec = match improvement::spec(area, &date, due.since.as_deref(), &data) {
            Ok(spec) => spec,
            Err(e) => return self.improvement_failed("could not write its spec", &e, warn),
        };
        // Never over a file this check didn't create, and none of its own left half written.
        let written = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => file.write_all(spec.as_bytes()).inspect_err(|_| {
                let _ = fs::remove_file(&path);
            }),
            Err(e) => Err(e),
        };
        if let Err(e) = written {
            let e = anyhow::Error::new(e).context(format!("cannot write {}", path.display()));
            return self.improvement_failed("could not write its spec", &e, warn);
        }
        let opts = task_queue::Options {
            repo_root: self.repo_root.clone(),
            repo: Some(self.repo.clone()),
        };
        let job = Job::Spec { file: name.clone() };
        let queued = task_queue::add(
            self.config_dir,
            &opts,
            &job,
            self.github,
            self.probe,
            self.env.now,
            self.env.sleep,
            out,
        );
        if let Err(e) = queued {
            // A job file written before the error still names the spec, so it stays.
            // A queue that can't be read can't tell, so the spec stays then too.
            if matches!(improvement::queued_spec(&self.base_dir, &name), Ok(false)) {
                let _ = fs::remove_file(&path);
            }
            return self.improvement_failed("could not queue its job", &e, warn);
        }
        let why = match (&due.since, due.prs) {
            (Some(since), Some(prs)) => format!("{prs} PRs merged since {since}"),
            _ => "no improvement review yet".to_owned(),
        };
        let line = format!(
            "improvement review of {} queued ({why}): {}",
            area.slug,
            job.label()
        );
        writeln!(out, "{line}")?;
        self.improvement_row(&line, warn)
    }

    /// The run data since `since`: the table of `sbxm task facts` and this repository's loop-log
    /// rows ([`improvement::run_data`] keeps those of that day or later). A source that can't be
    /// read whole is an error naming what it couldn't read: a log that can't be read, or a task
    /// record `task facts` skipped, so no spec is queued from incomplete data.
    fn run_data(&self, since: Option<&str>) -> Result<improvement::RunData> {
        let opts = task_facts::Options {
            since: since.unwrap_or("1970-01-01").to_owned(),
            until: None,
            json: false,
        };
        let (mut facts, mut warned) = (Vec::new(), Vec::new());
        task_facts::run(self.config_dir, &opts, self.now(), &mut facts, &mut warned)?;
        let skipped: Vec<String> = String::from_utf8_lossy(&warned)
            .lines()
            .map(|l| l.strip_prefix("warning: ").unwrap_or(l).trim().to_owned())
            .filter(|l| !l.is_empty())
            .collect();
        if let Some(first) = skipped.first() {
            let more = match skipped.len() {
                1 => String::new(),
                n => format!(" and {} more", n - 1),
            };
            anyhow::bail!("`sbxm task facts` could not read every task record: {first}{more}");
        }
        let mut logs = Vec::new();
        for (start, path) in loop_log::every(&self.base_dir)? {
            let read = fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            if loop_log::repository(&read) != Some(self.repo.as_str()) {
                continue;
            }
            let modified = fs::metadata(&path)
                .and_then(|m| m.modified())
                .with_context(|| format!("cannot read when {} was written", path.display()))?;
            let end = modified
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            logs.push(LoopLog {
                start,
                end,
                text: read,
            });
        }
        let anchors = improvement::anchors(&self.base_dir, &self.repo);
        Ok(improvement::run_data(
            &String::from_utf8_lossy(&facts),
            &logs,
            &anchors,
            since,
        ))
    }

    /// One toast for each review on the default branch that a spec the loop wrote asked for
    /// (its spec file is still in `specs_dir`), once: "Improvement review <slug> merged: sort its N
    /// findings".
    fn toast_merged_reviews(
        &mut self,
        listing: &Listing,
        specs_dir: &Path,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        for name in &listing.names {
            let Some((date, slug)) = improvement::review_name(name) else {
                continue;
            };
            let key = format!("improvement-merged-{name}");
            let shown = self
                .notify
                .as_ref()
                .is_none_or(|(_, shown)| shown.contains(&key));
            let ours = fs::symlink_metadata(specs_dir.join(improvement::spec_name(slug, date)))
                .is_ok_and(|m| m.is_file());
            if shown || !ours {
                continue;
            }
            let path = format!("{}/{name}", improvement::DIR);
            let text = self.github.file_text(&self.repo, &listing.branch, &path)?;
            let n = improvement::findings(&text);
            let title = format!("Improvement review {slug} merged");
            let note = Notification {
                key,
                body: format!("{title}: sort its {n} findings"),
                title,
                click: self.log_click(),
            };
            self.notify(note, out, warn)?;
        }
        Ok(())
    }

    /// A check that failed at `what`: a warning and a line in the loop's own log; it is tried
    /// again in an hour.
    fn improvement_failed(
        &mut self,
        what: &str,
        e: &anyhow::Error,
        warn: &mut dyn Write,
    ) -> Result<()> {
        let line = format!(
            "the improvement review check {what} ({}); it is tried again in an hour, the loop \
             goes on",
            first_line(&format!("{e:#}"))
        );
        self.improvement_log(&line, warn)
    }

    /// `line` logged as [`Self::improvement_log`] does, once a run.
    fn improvement_note_once(&mut self, line: String, warn: &mut dyn Write) -> Result<()> {
        if self.improvement_noted.insert(line.clone()) {
            self.improvement_log(&line, warn)?;
        }
        Ok(())
    }

    /// One warning, and one line in the loop's own log ([`queue::append_note`]). A log that can't
    /// be written is a warning once; the loop goes on.
    fn improvement_log(&mut self, line: &str, warn: &mut dyn Write) -> Result<()> {
        writeln!(warn, "warning: {line}")?;
        self.improvement_row(line, warn)
    }

    /// `line` in the loop's own log ([`queue::append_note`]). A log that can't be written is a
    /// warning once; the loop goes on.
    fn improvement_row(&mut self, line: &str, warn: &mut dyn Write) -> Result<()> {
        if let Err(e) = queue::append_note(&self.log, &self.repo, line) {
            self.warn_once(
                warn,
                format!("{e:#}; the loop goes on without that line in its log"),
            )?;
        }
        Ok(())
    }
}
