//! Ending a task (spec §4, decisions 153, 158): discarding one (`task rm`, `task start
//! --restart`) and, further down, handing a ready one to GitHub (`task finish`): a new PR, or
//! more commits on the open PR the task continues (decision 169 (e)).
//!
//! Removal only ever touches a task's own folders, whose paths are built from a validated id and
//! checked again on disk before anything is deleted: never a link, never a folder that doesn't
//! resolve to its expected place under the base dir.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::lifetime::{self, Removal};
use super::lock::ProcessLock;
use super::machine;
use super::pipeline::{self, Prepared};
use super::record::{self, OpenFinding, Process, ProcessProbe, Record};
use super::repo;
use super::review::{self, COMMENT_CAP, defang, defang_mentions};
use super::risk;
use super::summary;
use crate::backend::{Location, SandboxBackend};
use crate::confirm::Confirm;
use crate::github::{GitHubBackend, ListedPr, PrRequest};
use crate::run::results::now;

/// Everything `task rm` would delete, worked out before anything is.
#[derive(Debug)]
pub struct RemovalPlan {
    pub id: String,
    base_dir: PathBuf,
    /// Worker's then reviewer's, as `task.json` names them; for a cloud task, every sandbox its
    /// `sandboxes` list has with no `removed_at` (milestone 4 slice 6, decision 265).
    pub sandboxes: Vec<String>,
    /// Where the sandboxes are, as `task.json` records it (decision 253): never the config's.
    pub location: Location,
    /// The folders that exist, the task folder last.
    pub folders: Vec<PathBuf>,
    /// Set when `task.json` couldn't be read, so no sandbox names are known.
    pub note: Option<String>,
    /// The cloud sandbox of an ambiguous create `--forget-ambiguous` drops from the task's
    /// record before anything is removed ([`forget_in_record`]), with no cloud call for it
    /// (decision 268).
    pub forget: Option<String>,
    /// The task's mutation lock (issue 58), taken before its record was read and held until the
    /// plan is dropped; `None` when the task has no `.sbxm/tasks/<id>/` folder to lock.
    lock: Option<ProcessLock>,
}

impl RemovalPlan {
    /// The exact things that will go, one per line, for the confirmation.
    pub fn listing(&self) -> String {
        let mut lines: Vec<String> = self
            .sandboxes
            .iter()
            .map(|s| match self.location {
                Location::Local => format!("  sandbox {s}"),
                Location::Cloud => format!("  cloud sandbox {s}"),
            })
            .collect();
        lines.extend(self.folders.iter().map(|f| format!("  {}", f.display())));
        if let Some(name) = &self.forget {
            lines.push(format!(
                "  note: cloud sandbox {name}, whose create sbxm never heard return, is only \
                 dropped from task.json, not removed; look with `sbx --cloud ls`"
            ));
        }
        if let Some(note) = &self.note {
            lines.push(format!("  note: {note}"));
        }
        lines.join("\n")
    }
}

/// What the removal did, in words.
#[derive(Debug, Default)]
pub struct RemovalReport {
    pub removed: Vec<String>,
    /// Each says what stayed and why.
    pub stayed: Vec<String>,
}

impl RemovalReport {
    pub fn is_clean(&self) -> bool {
        self.stayed.is_empty()
    }
}

/// The folders of task `id`, relative to the base dir, clones first and the task folder last.
fn task_folders(id: &str) -> Vec<Vec<String>> {
    let tasks = |name: String| vec!["tasks".to_owned(), name];
    vec![
        tasks(id.to_owned()),
        tasks(format!("{id}-review")),
        tasks(format!("{id}-gates")),
        vec![".sbxm".to_owned(), "tasks".to_owned(), id.to_owned()],
    ]
}

fn schema_too_new(path: &Path) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v["schema"].as_u64())
        .is_some_and(|schema| schema > u64::from(record::SCHEMA))
}

/// A sandbox name from `task.json` is only removed if it is one of this task's (`task.json` is a
/// file on disk, and the name goes to `sbx rm`).
fn check_sandbox_name(id: &str, name: &str) -> Result<()> {
    let prefix = format!("sbxm-task-{id}-");
    let tail = name.strip_prefix(&prefix);
    let plain = tail.is_some_and(|t| {
        !t.is_empty()
            && t.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    });
    if !plain {
        bail!(
            "task.json of {id} names sandbox {name:?}, which isn't one of its sandboxes; \
             fix or delete {id}'s task folder by hand"
        );
    }
    Ok(())
}

/// Whether any folder of task `id` exists (a task, or what a failed start left behind).
pub fn exists(base_dir: &Path, id: &str) -> bool {
    record::is_valid_id(id)
        && task_folders(id).iter().any(|rel| {
            rel.iter()
                .fold(base_dir.to_path_buf(), |p, s| p.join(s))
                .symlink_metadata()
                .is_ok()
        })
}

/// The ids of PR `pr`'s fix tasks (`fix-<pr>-<head>`, issue 485), whatever head each was made
/// for, sorted: every name under `<base>/.sbxm/tasks/` and `<base>/tasks/` that is one, or is one
/// with a reviewer's or gates' `-review`/`-gates` suffix, so a task a failed start left only a
/// workspace of is found too. Only names are read; [`exists`] and [`plan_removal`] check the
/// folders.
pub fn fix_tasks_of(base_dir: &Path, pr: u32) -> Result<Vec<String>> {
    let prefix = format!("fix-{pr}-");
    let mut ids = Vec::new();
    for root in [record::tasks_root(base_dir), base_dir.join("tasks")] {
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", root.display())),
        };
        for entry in entries {
            let name = entry?.file_name().to_string_lossy().into_owned();
            let id = ["-review", "-gates"]
                .iter()
                .find_map(|suffix| name.strip_suffix(suffix))
                .filter(|id| record::is_valid_id(id))
                .unwrap_or(&name);
            if id.starts_with(&prefix) && record::is_valid_id(id) {
                ids.push(id.to_owned());
            }
        }
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// A fix task's result lives in its `repo.git` until `task fix` pushes it onto its PR's branch
/// (issue 485): refused while the task isn't `finished` (pushed) and its branch has commits past
/// the PR head it started from, or when that can't be told, so `task rm --pr` never deletes the
/// only copy by accident; the caller skips this for `--force`. Only reads.
pub fn check_fix_result_pushed(base_dir: &Path, id: &str) -> Result<()> {
    let force = "pass --force to delete it anyway";
    let meta = record::task_dir(base_dir, id);
    let repo_git = meta.join("repo.git");
    // Checked without following it (decision 159), as `task rm --merged` does.
    if fs::symlink_metadata(&repo_git).is_err() {
        return Ok(());
    }
    let task = record::read(&meta.join("task.json"))
        .with_context(|| format!("cannot tell whether task {id}'s result was pushed; {force}"))?;
    if task.stage == record::Stage::Finished {
        return Ok(());
    }
    let number = task.number;
    let ahead = check_own_folder(base_dir, &repo_git)
        .and_then(|()| task.commits_ahead(&repo_git))
        .with_context(|| {
            format!(
                "whether task {id}'s result reached PR #{number}'s branch can't be checked; \
                 {force}"
            )
        })?;
    if ahead > 0 {
        bail!(
            "task {id} has {ahead} commit(s) on {} that were never pushed to PR #{number}'s \
             branch; they exist only in {}, which `task rm` deletes; {force}",
            task.branch,
            repo_git.display()
        );
    }
    Ok(())
}

/// Checks that `id` can be discarded and lists what that removes. The id is validated before any
/// path is built from it; a task whose process is alive is refused.
pub fn plan_removal(base_dir: &Path, id: &str, probe: &dyn ProcessProbe) -> Result<RemovalPlan> {
    plan_removal_forgetting(base_dir, id, probe, false)
}

/// [`plan_removal`], with `--forget-ambiguous` when `forget`: a cloud create that was started
/// and never recorded as returned (decision 268) is refused, naming the manual commands, unless
/// `forget`, when the plan drops it from `task.json` first and never removes it or its secrets.
pub fn plan_removal_forgetting(
    base_dir: &Path,
    id: &str,
    probe: &dyn ProcessProbe,
    forget: bool,
) -> Result<RemovalPlan> {
    if !record::is_valid_id(id) {
        bail!("{id:?} isn't a task id; task ids look like issue-41 or pr-7");
    }
    let meta = record::task_dir(base_dir, id);
    let folders: Vec<PathBuf> = task_folders(id)
        .into_iter()
        .map(|rel| rel.iter().fold(base_dir.to_path_buf(), |p, s| p.join(s)))
        .filter(|p| p.symlink_metadata().is_ok())
        .collect();
    if folders.is_empty() {
        bail!("no task {id}; run `sbxm task status` to list the tasks");
    }
    // Before the record is read: a task another live command holds is refused (issue 58).
    let lock = pipeline::lock_task(base_dir, id, probe)?;

    let json = meta.join("task.json");
    let mut sandboxes = Vec::new();
    let mut location = Location::Local;
    let mut note = None;
    let mut forgotten = None;
    if json.is_file() && schema_too_new(&json) {
        // Only a newer sbxm knows what is in it; don't delete a task this one can't read.
        record::read(&json)?;
    }
    match record::read(&json) {
        Ok(task) => {
            let interrupted = task.is_interrupted(probe);
            let state = machine::State {
                stage: task.stage,
                status: task.status,
                interrupted,
                kind: task.kind,
            };
            if !machine::verdict(&state, machine::Event::Rm) {
                bail!(
                    "task {id} is running (stage {}); wait for it to finish, or stop it, then run `sbxm task rm` again",
                    task.stage.name()
                );
            }
            if let Some(name) = task.ambiguous_create() {
                if !forget {
                    bail!(
                        "task {id} is left as it is: {}",
                        lifetime::ambiguous_create(name, lifetime::FORGET_BY_RM)
                    );
                }
                forgotten = Some(name.to_owned());
            }
            sandboxes = sandbox_names(id, &task)?;
            location = task.location;
        }
        Err(e) => {
            note = Some(format!(
                "task.json can't be read ({e:#}), so its sandboxes can't be named; check `sbx ls`, and `sbx --cloud ls` for a cloud task, for sbxm-task-{id}-*"
            ));
        }
    }
    Ok(RemovalPlan {
        id: id.to_owned(),
        base_dir: base_dir.to_path_buf(),
        sandboxes,
        location,
        folders,
        note,
        forget: forgotten,
        lock,
    })
}

/// The sandboxes removing `task` removes.
fn sandbox_names(id: &str, task: &Record) -> Result<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    // A cloud task's sandboxes are the ones it owns (P5 (d), decision 263; decision 268): each
    // lifetime entry still open, its create having returned. A sandbox removed at the task's stop
    // is gone already, and no other name is the task's: a create that returned an error made
    // none, one refused because the cloud already had the name never asked, and an ambiguous one
    // (started, never heard back from) is the user's to look at, refused or forgotten by
    // `plan_removal_forgetting` (slice 1 review M-1).
    if task.location == Location::Cloud {
        for name in task.open_sandboxes() {
            check_sandbox_name(id, &name)?;
            names.push(name);
        }
        return Ok(names);
    }
    for agent in [&task.worker, &task.reviewer].into_iter().flatten() {
        check_sandbox_name(id, &agent.sandbox)?;
        names.push(agent.sandbox.clone());
    }
    Ok(names)
}

/// The sandboxes a cloud task still owns (a removal at its stop that failed; never an ambiguous
/// create, decision 268), each checked to be one of the task's own before any goes
/// to `sbx --cloud rm`; none for a local task (milestone 4 slice 6).
pub fn open_cloud_sandboxes(task: &Record) -> Result<Vec<String>> {
    match task.location {
        Location::Local => Ok(Vec::new()),
        Location::Cloud => sandbox_names(&task.id, task),
    }
}

/// Refuses a folder that is a link, or whose real place isn't `<base>/<rel...>`.
pub(crate) fn check_own_folder(base_dir: &Path, dir: &Path) -> Result<()> {
    if dir.symlink_metadata()?.file_type().is_symlink() {
        bail!(
            "{} is a symlink or junction; remove the link yourself if you want it gone",
            dir.display()
        );
    }
    let relative = dir.strip_prefix(base_dir)?;
    let expected = fs::canonicalize(base_dir)?.join(relative);
    let real = fs::canonicalize(dir)?;
    if real != expected {
        bail!(
            "{} resolves to {}, not {}; not deleting it",
            dir.display(),
            real.display(),
            expected.display()
        );
    }
    Ok(())
}

/// Removes `plan`'s sandboxes where its record says they are (decision 253), best effort, into
/// `report`. The recorded location is selected here, from `plan`, whatever `backend` the caller
/// holds: a `--restart` into the cloud passes the new task's cloud backend, and the old task's
/// local sandbox must still go through the local `sbx rm`. A cloud task's (milestone 4 slice 6)
/// go through the retried removal, `sleep` waiting between tries (P6, decision 262(e)), one
/// already gone counting as removed; then the secrets the cloud lists as scoped to each (slice
/// 4), whatever its removal said. Returns `false` when a cloud sandbox may still be there: the
/// task's record is then its only trace.
fn remove_sandboxes(
    plan: &RemovalPlan,
    backend: &dyn SandboxBackend,
    sleep: &dyn Fn(Duration),
    report: &mut RemovalReport,
) -> bool {
    let backend = backend.located(plan.location);
    if plan.location == Location::Local {
        for sandbox in &plan.sandboxes {
            match crate::commands::rm::remove_sandbox(backend, sandbox) {
                Ok(()) => report.removed.push(format!("sandbox {sandbox}")),
                Err(e) => report
                    .stayed
                    .push(format!("sandbox {sandbox} stayed: {e:#}")),
            }
        }
        return true;
    }
    let cloud = backend;
    let mut all_gone = true;
    for sandbox in &plan.sandboxes {
        match lifetime::remove_sandbox(cloud, sandbox, sleep) {
            Removal::Removed => report.removed.push(format!("cloud sandbox {sandbox}")),
            Removal::AlreadyGone => report
                .removed
                .push(format!("cloud sandbox {sandbox} (already gone)")),
            Removal::Failed(e) => {
                all_gone = false;
                report.stayed.push(format!(
                    "cloud sandbox {sandbox}: {e}; remove it with `sbx --cloud rm {sandbox} -f`"
                ));
            }
        }
        remove_scoped_secrets(cloud, sandbox, report);
    }
    all_gone
}

/// The secrets the cloud lists as scoped to `sandbox` (slice 4, by scope: `task.json` holds none
/// of them), each removed, into `report`; a failure names the command that finishes it by hand.
fn remove_scoped_secrets(cloud: &dyn SandboxBackend, sandbox: &str, report: &mut RemovalReport) {
    let names = match cloud.scoped_secrets(sandbox) {
        Ok(names) => names,
        Err(e) => {
            report.stayed.push(format!(
                "the secrets scoped to {sandbox}: the cloud's list couldn't be read ({e:#}); list \
                 them with `sbx --cloud secret ls` and remove each with `sbx --cloud secret rm \
                 <name> --sandbox {sandbox} -f`"
            ));
            return;
        }
    };
    for name in names {
        match cloud.remove_secret(&name, sandbox) {
            Ok(()) => report
                .removed
                .push(format!("scoped secret {name} of {sandbox}")),
            Err(e) => report.stayed.push(format!(
                "scoped secret {name} of {sandbox}: {e:#}; remove it with `sbx --cloud secret rm \
                 {name} --sandbox {sandbox} -f`"
            )),
        }
    }
}

/// Removes what `plan` lists, best effort: sandboxes first (where the task's record says, see
/// [`remove_sandboxes`]), then the clones, then the task folder, which stays if anything else did
/// (it holds the sandbox names a second `rm` needs). When a cloud sandbox may still be there,
/// nothing else is touched: its folder holds the only record of a sandbox that may be running.
pub fn remove(plan: &RemovalPlan, backend: &dyn SandboxBackend) -> RemovalReport {
    remove_with_sleep(plan, backend, &std::thread::sleep)
}

/// [`remove`], with `sleep` called for each wait between a cloud sandbox's removal retries (P6
/// (a)), and between a folder delete's tries ([`delete_folder`]), instead of really sleeping.
pub fn remove_with_sleep(
    plan: &RemovalPlan,
    backend: &dyn SandboxBackend,
    sleep: &dyn Fn(Duration),
) -> RemovalReport {
    remove_with_delete(plan, backend, sleep, &|folder| fs::remove_dir_all(folder))
}

/// How many times in all a folder's delete is tried while Windows refuses it as in use, and how
/// long it waits between two tries (issue 503): a file another process still has open (os error
/// 32) can let `remove_dir_all` delete part of the folder and then fail, and a later try
/// continues from what is left.
const DELETE_ATTEMPTS: u32 = 5;
const DELETE_PAUSE: Duration = Duration::from_secs(1);

/// `delete` of `folder`, tried again after a sharing violation (os error 32) or a
/// `PermissionDenied`, up to [`DELETE_ATTEMPTS`] in all with [`DELETE_PAUSE`] between two tries,
/// like the lock's `retry_denied` (issue 441). Any other error, or the last try's, is returned
/// as it is.
fn delete_folder(
    folder: &Path,
    delete: &dyn Fn(&Path) -> std::io::Result<()>,
    sleep: &dyn Fn(Duration),
) -> std::io::Result<()> {
    let mut attempt = 1;
    loop {
        match delete(folder) {
            Err(e)
                if (e.raw_os_error() == Some(32)
                    || e.kind() == std::io::ErrorKind::PermissionDenied)
                    && attempt < DELETE_ATTEMPTS =>
            {
                sleep(DELETE_PAUSE);
                attempt += 1;
            }
            outcome => return outcome,
        }
    }
}

/// [`remove_with_sleep`], with `delete` in place of `fs::remove_dir_all` for each folder, so
/// tests can make it fail.
pub fn remove_with_delete(
    plan: &RemovalPlan,
    backend: &dyn SandboxBackend,
    sleep: &dyn Fn(Duration),
    delete: &dyn Fn(&Path) -> std::io::Result<()>,
) -> RemovalReport {
    let mut report = RemovalReport::default();
    // `--forget-ambiguous` drops the ambiguous create from `task.json` before anything is removed
    // (PR 322 round 2, M-2): a removal that stops part way leaves a record a plain `task rm` can
    // retry. A drop that can't be saved removes nothing.
    if let Some(name) = &plan.forget {
        if let Err(e) = forget_in_record(plan) {
            report.stayed.push(format!(
                "cloud sandbox {name}'s ambiguous create couldn't be dropped from task.json \
                 ({e:#}), so nothing of task {} was removed",
                plan.id
            ));
            return report;
        }
        report.removed.push(format!(
            "cloud sandbox {name}'s ambiguous create, forgotten (the sandbox and its secrets left \
             alone)"
        ));
    }
    if !remove_sandboxes(plan, backend, sleep, &mut report) {
        report.stayed.extend(plan.folders.iter().map(|folder| {
            format!(
                "{}: a cloud sandbox of the task may still be running, and the task's record is \
                 its only trace; the task is left as it is",
                folder.display()
            )
        }));
        return report;
    }
    let meta_dir = record::task_dir(&plan.base_dir, &plan.id);
    for folder in &plan.folders {
        if *folder == meta_dir && !report.is_clean() {
            report.stayed.push(format!(
                "{} stayed, so a second `sbxm task rm` can still name the sandboxes",
                folder.display()
            ));
            continue;
        }
        let done = check_own_folder(&plan.base_dir, folder).and_then(|()| {
            delete_folder(folder, delete, sleep)
                .map_err(|e| anyhow::anyhow!("cannot delete {}: {e}", folder.display()))
        });
        match done {
            Ok(()) => report.removed.push(folder.display().to_string()),
            Err(e) => report
                .stayed
                .push(format!("{} stayed: {e:#}", folder.display())),
        }
    }
    report
}

/// Drops `plan`'s forgotten ambiguous create (`--forget-ambiguous`, decision 268) from the task's
/// `task.json`, with no cloud call for it; nothing to do once the record no longer has it, as
/// after `task rm --merged` dropped it before archiving. A different ambiguous create recorded
/// since the plan was made is refused.
pub fn forget_in_record(plan: &RemovalPlan) -> Result<()> {
    let Some(name) = &plan.forget else {
        return Ok(());
    };
    let meta = record::task_dir(&plan.base_dir, &plan.id);
    check_own_folder(&plan.base_dir, &meta)?;
    let mut task = record::read(&meta.join("task.json"))?;
    match task.ambiguous_create() {
        None => Ok(()),
        Some(recorded) if recorded == name => record::write_change(&meta, &mut task, |t| {
            t.forget_ambiguous();
        }),
        Some(recorded) => bail!(
            "task {} now records the ambiguous create of {recorded}, not {name}; run `sbxm task \
             rm` again",
            plan.id
        ),
    }
}

/// Where a set-aside task's folders live while a `--restart` discard is undoable (decision 185):
/// same drive as the base dir, so moving a task here and back is a rename; outside `tasks/` and
/// `.sbxm/tasks/`, so `task status` and `task rm` never list or touch it.
fn replaced_root(base_dir: &Path, id: &str) -> PathBuf {
    base_dir.join(".sbxm").join("tasks-replaced").join(id)
}

/// Whether a leftover set-aside copy of `id` exists: an earlier `--restart` discarded it but was
/// never settled one way or the other, most likely because it, or the process running it, was
/// interrupted (decision 185 point 4). A tree of empty folders only is not one: it is what a
/// restore that put every folder back leaves when it can't delete the tree (issue #224), and
/// [`check_no_leftover`] deletes it.
pub fn has_leftover(base_dir: &Path, id: &str) -> bool {
    let dir = replaced_root(base_dir, id);
    dir.symlink_metadata().is_ok() && !only_empty_folders(&dir)
}

/// Refuses a `--restart` of `id` while [`has_leftover`] holds, before anything else about it is
/// checked: the folder and the two ways out (moving it back by hand, or deleting it) are named.
/// There is no command of its own for the leftover (decision 185 point 4). An empty remnant
/// (issue #224) is deleted instead, folder by folder and never through a link; the refusal for one
/// that can't be says it is safe to delete.
pub fn check_no_leftover(base_dir: &Path, id: &str) -> Result<()> {
    let dir = replaced_root(base_dir, id);
    if dir.symlink_metadata().is_err() {
        return Ok(());
    }
    check_own_folder(base_dir, &dir).with_context(|| {
        format!(
            "an earlier `--restart` of task {id} left {} behind",
            dir.display()
        )
    })?;
    if !only_empty_folders(&dir) {
        bail!(
            "an earlier `--restart` of task {id} left its old copy at {}, never settled; move it \
             back into place yourself, or delete it, before restarting {id} again",
            dir.display()
        );
    }
    remove_empty_folders(&dir).map_err(|e| {
        anyhow::anyhow!(
            "{} is an empty folder left from an earlier restore of task {id}, safe to delete, but \
             it could not be deleted ({e}); delete it yourself, then restart {id} again",
            dir.display()
        )
    })
}

/// Whether `dir` is a folder, not a link, holding nothing but folders like it at any depth.
/// Anything that can't be read counts as holding something.
fn only_empty_folders(dir: &Path) -> bool {
    let is_dir = dir.symlink_metadata().is_ok_and(|m| m.file_type().is_dir());
    is_dir
        && fs::read_dir(dir).is_ok_and(|mut entries| {
            entries.all(|entry| entry.is_ok_and(|e| only_empty_folders(&e.path())))
        })
}

/// Deletes `dir` and the folders in it, deepest first, with `remove_dir` only: it deletes nothing
/// but an empty folder, so a file or a link anywhere in the tree stops it with an error, and it
/// never reads through a link, `dir` itself included.
fn remove_empty_folders(dir: &Path) -> std::io::Result<()> {
    if !dir.symlink_metadata()?.file_type().is_dir() {
        return Err(std::io::Error::other("not a folder"));
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            remove_empty_folders(&entry.path())?;
        }
    }
    fs::remove_dir(dir)
}

/// A task's folders, set aside instead of deleted so a `--restart` discard can be undone
/// (decision 185). [`set_aside_many`] builds one of these per task: its sandboxes are already
/// gone (removed the same way [`remove`] removes them; that part is not undone), but each folder
/// [`plan_removal`] found is moved to [`replaced_root`] rather than deleted. [`SetAside::restore`]
/// moves every one of them back; [`SetAside::settle`] deletes them for good, once the task that
/// replaced this one no longer needs them to fall back to.
#[derive(Debug)]
pub struct SetAside {
    id: String,
    base_dir: PathBuf,
    /// Paths relative to the base dir that were actually moved, so `restore`/`settle` touch
    /// exactly what `set_aside_many` moved and nothing it left alone.
    moved: Vec<PathBuf>,
}

impl SetAside {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Where this task's old copy currently sits.
    pub fn folder(&self) -> PathBuf {
        replaced_root(&self.base_dir, &self.id)
    }

    fn pair(&self, rel: &Path) -> (PathBuf, PathBuf) {
        (self.base_dir.join(rel), self.folder().join(rel))
    }

    /// Moves every folder back to where it was, so the old task is exactly as it was before
    /// [`set_aside_many`] moved it aside (its sandboxes are not restored; decision 185 point 2).
    /// If one folder can't move back, whatever was already moved back in this call is moved aside
    /// again, so the old task stays entirely in one place or the other, never split between them.
    /// Once all are back, the emptied set-aside tree is deleted; if that fails the task is still
    /// back, and the returned warning names the tree (issue #224).
    pub fn restore(&self) -> Result<Option<String>> {
        let mut done: Vec<&PathBuf> = Vec::new();
        for rel in &self.moved {
            let (original, replaced) = self.pair(rel);
            if let Some(parent) = original.parent()
                && let Err(e) = fs::create_dir_all(parent)
            {
                self.undo_restore(&done);
                return Err(e).with_context(|| format!("cannot create {}", parent.display()));
            }
            // Checked here, not left to `rename`: what a rename does to something in the way
            // depends on the platform (Windows replaces a plain file, Linux an empty folder), and
            // nothing more may be deleted.
            let moved_back = if original.symlink_metadata().is_ok() {
                Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "something is already there",
                ))
            } else {
                fs::rename(&replaced, &original)
            };
            if let Err(e) = moved_back {
                self.undo_restore(&done);
                return Err(e).with_context(|| {
                    format!(
                        "cannot move {} back to {}",
                        replaced.display(),
                        original.display()
                    )
                });
            }
            done.push(rel);
        }
        let dir = self.folder();
        // Nothing to clean when no folder is there: never created, or something else stands in
        // its place (a file, a link), which is not this restore's to judge or delete.
        if !dir.symlink_metadata().is_ok_and(|m| m.file_type().is_dir()) {
            return Ok(None);
        }
        Ok(remove_empty_folders(&dir).err().map(|e| {
            format!(
                "the old task {} was put back, but its emptied set-aside folder {} could not be \
                 deleted ({e}); the next `--restart` of {} deletes it, or delete it yourself",
                self.id,
                dir.display(),
                self.id
            )
        }))
    }

    fn undo_restore(&self, done: &[&PathBuf]) {
        for rel in done.iter().rev() {
            let (original, replaced) = self.pair(rel);
            let _ = fs::rename(&original, &replaced);
        }
    }

    /// Deletes the set-aside copy for good: the task that replaced it no longer needs it.
    pub fn settle(&self) -> Result<()> {
        let dir = self.folder();
        fs::remove_dir_all(&dir).with_context(|| format!("cannot delete {}", dir.display()))
    }
}

/// What a `--restart` discard tells the caller once it is known that the task which was to
/// replace `aside` did not finish preparing: `start_failed` is why. If putting the old task back
/// succeeds, the returned error is `start_failed` plus a note that it's back; if putting it back
/// also fails, the error names both failures and the folder it is still at, so nothing more is
/// deleted (decision 185 point 4).
pub fn restore_after_failed_start(aside: &SetAside, start_failed: anyhow::Error) -> anyhow::Error {
    match aside.restore() {
        Ok(None) => start_failed.context(format!("the old task {} was put back", aside.id())),
        Ok(Some(warning)) => start_failed.context(warning),
        Err(restore_failed) => anyhow::anyhow!(
            "the start failed ({start_failed:#}), and putting the old task {} back failed too \
             ({restore_failed:#}); it is still set aside at {}",
            aside.id(),
            aside.folder().display()
        ),
    }
}

/// [`discard_many`], but moves each task's folders to [`replaced_root`] instead of deleting them
/// (decision 185): the confirmation and the sandbox removal are the same. A caller gets back one
/// [`SetAside`] per task and decides, once it knows whether the task that replaced it finished
/// preparing, whether to [`SetAside::restore`] or [`SetAside::settle`] it; nothing here decides
/// that. A local sandbox that can't be removed is reported but doesn't stop the rest (as
/// [`remove`] does): it is gone or left behind either way, only the folders are undoable. A
/// cloud sandbox that may still be there stops the batch before its task is set aside, and
/// every task already set aside in this call goes back (milestone 4 slice 6): the task's
/// record is the only trace of a sandbox that may still be running.
pub fn set_aside_many(
    base_dir: &Path,
    ids: &[String],
    yes: bool,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    confirm: &dyn Confirm,
    out: &mut dyn Write,
) -> Result<Vec<SetAside>> {
    set_aside_many_with_sleep(
        base_dir,
        ids,
        yes,
        backend,
        probe,
        confirm,
        out,
        &std::thread::sleep,
        &|| Ok(()),
    )
}

/// [`set_aside_many`], with `sleep` called for each wait between a cloud sandbox's removal
/// retries (P6 (a)) instead of really sleeping. A cloud sandbox that still can't be removed
/// stops the restart before its task is moved, and every task already set aside goes back.
/// `check` runs once every task is planned, under their locks (issue 58), and before the question:
/// an error refuses the restart with nothing touched.
#[allow(clippy::too_many_arguments)]
pub fn set_aside_many_with_sleep(
    base_dir: &Path,
    ids: &[String],
    yes: bool,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    confirm: &dyn Confirm,
    out: &mut dyn Write,
    sleep: &dyn Fn(Duration),
    check: &dyn Fn() -> Result<()>,
) -> Result<Vec<SetAside>> {
    let mut plans = ids
        .iter()
        .map(|id| plan_removal(base_dir, id, probe))
        .collect::<Result<Vec<_>>>()?;
    check()?;
    let listing = plans
        .iter()
        .map(RemovalPlan::listing)
        .collect::<Vec<_>>()
        .join("\n");
    let names = ids.join(", ");
    let what = if ids.len() == 1 {
        format!("task {names}")
    } else {
        format!("{} tasks ({names})", ids.len())
    };
    if !yes {
        if !confirm.is_interactive() {
            bail!(
                "refusing to restart {what} without a terminal; pass --yes to restart it:\n{listing}"
            );
        }
        if !confirm.confirm(&format!(
            "Restart {what}: discard these sandboxes and start over?\n{listing}\n"
        ))? {
            bail!("restarting {what} cancelled; nothing was deleted");
        }
    }
    let mut result: Vec<SetAside> = Vec::new();
    for plan in &mut plans {
        let mut report = RemovalReport::default();
        let all_gone = remove_sandboxes(plan, backend, sleep, &mut report);
        let lines = report.removed.iter().map(|l| format!("removed {l}"));
        let lines = lines.chain(
            report
                .stayed
                .iter()
                .map(|l| format!("could not remove {l}")),
        );
        for line in lines {
            if let Err(e) = writeln!(out, "{line}") {
                return Err(undo_set_asides(&result, e.into()));
            }
        }
        // A cloud sandbox that may still run keeps its task where it is (milestone 4 slice 6).
        if !all_gone {
            let failed = anyhow::anyhow!(
                "a cloud sandbox of task {} could not be removed, so the task is left as it is; \
                 remove the sandbox as said above, then restart again",
                plan.id
            );
            return Err(undo_set_asides(&result, failed));
        }
        match set_aside_one(base_dir, plan) {
            Ok(aside) => {
                // Its lock moved aside with its folder, and is released there: a task put back
                // later comes back unlocked.
                if let Some(mut lock) = plan.lock.take() {
                    let (_, meta) = aside.pair(&Path::new(".sbxm").join("tasks").join(&plan.id));
                    lock.moved_to(&meta);
                }
                result.push(aside);
            }
            Err(e) => return Err(undo_set_asides(&result, e)),
        }
    }
    Ok(result)
}

/// Restores every task already set aside in this [`set_aside_many`] call, best-effort, when
/// something else in the same call fails partway through the batch (another task's set-aside, or
/// the status output in between): the one path that un-does an accumulated [`SetAside`] list, so
/// a later fallible step added to the loop rolls back the same way instead of needing its own copy.
/// Returns the error for the caller: `failed` (why the batch stopped) as it is when every task
/// went back, or else `failed` plus, for each task that could not be put back, why and the folder
/// it is still set aside at (decision 185 point 4). Every restore is tried either way.
fn undo_set_asides(result: &[SetAside], failed: anyhow::Error) -> anyhow::Error {
    let stranded: Vec<String> = result.iter().filter_map(stranded).collect();
    if stranded.is_empty() {
        return failed;
    }
    anyhow::anyhow!("{failed:#}, and {}", stranded.join(", and "))
}

/// [`set_aside_many`] for one task.
pub fn set_aside(
    base_dir: &Path,
    id: &str,
    yes: bool,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    confirm: &dyn Confirm,
    out: &mut dyn Write,
) -> Result<SetAside> {
    set_aside_with_sleep(
        base_dir,
        id,
        yes,
        backend,
        probe,
        confirm,
        out,
        &std::thread::sleep,
        &|| Ok(()),
    )
}

/// [`set_aside`], with `sleep` and `check` as for [`set_aside_many_with_sleep`].
#[allow(clippy::too_many_arguments)]
pub fn set_aside_with_sleep(
    base_dir: &Path,
    id: &str,
    yes: bool,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    confirm: &dyn Confirm,
    out: &mut dyn Write,
    sleep: &dyn Fn(Duration),
    check: &dyn Fn() -> Result<()>,
) -> Result<SetAside> {
    let ids = [id.to_owned()];
    let mut all = set_aside_many_with_sleep(
        base_dir, &ids, yes, backend, probe, confirm, out, sleep, check,
    )?;
    Ok(all.remove(0))
}

/// Moves every folder `plan` lists into [`replaced_root`], rolling back if one of them fails
/// partway so the old task stays entirely where it started instead of being split.
fn set_aside_one(base_dir: &Path, plan: &RemovalPlan) -> Result<SetAside> {
    // Every folder is checked before any is moved: one that is refused leaves the task untouched.
    let mut rels: Vec<PathBuf> = Vec::new();
    for folder in &plan.folders {
        check_own_folder(base_dir, folder)?;
        let rel = folder
            .strip_prefix(base_dir)
            .with_context(|| format!("{} isn't under {}", folder.display(), base_dir.display()))?;
        rels.push(rel.to_path_buf());
    }
    let mut aside = SetAside {
        id: plan.id.clone(),
        base_dir: base_dir.to_path_buf(),
        moved: Vec::new(),
    };
    for rel in rels {
        let (original, replaced) = aside.pair(&rel);
        let moved = match replaced.parent() {
            Some(parent) => fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display())),
            None => Ok(()),
        }
        .and_then(|()| {
            fs::rename(&original, &replaced).with_context(|| {
                format!(
                    "cannot move {} to {}",
                    original.display(),
                    replaced.display()
                )
            })
        });
        if let Err(e) = moved {
            return Err(match stranded(&aside) {
                Some(note) => anyhow::anyhow!("{e:#}, and {note}"),
                None => e,
            });
        }
        aside.moved.push(rel);
    }
    Ok(aside)
}

/// Puts `aside` back; `None` when that worked, or else why it didn't and the folder the old task
/// is still set aside at (decision 185 point 4), or the folder left behind when only deleting the
/// emptied set-aside tree failed (issue #224), for the caller's error to carry.
fn stranded(aside: &SetAside) -> Option<String> {
    let restore_failed = match aside.restore() {
        Ok(warning) => return warning,
        Err(e) => e,
    };
    Some(format!(
        "putting the old task {} back failed too ({restore_failed:#}); it is still set aside at {}",
        aside.id(),
        aside.folder().display()
    ))
}

/// Screen output of a `--restart` command (decision 185): writing to it never fails, so no step
/// between setting an old task aside and settling or restoring it can be cut short by output.
/// The first write error is kept, later output is dropped, and [`after_output`] reports the
/// error once the command is done.
pub struct Unfailing<'a> {
    inner: &'a mut dyn Write,
    failed: Option<std::io::Error>,
}

impl<'a> Unfailing<'a> {
    pub fn new(inner: &'a mut dyn Write) -> Self {
        Self {
            inner,
            failed: None,
        }
    }

    pub fn into_error(self) -> Option<std::io::Error> {
        self.failed
    }
}

impl Write for Unfailing<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.failed.is_none()
            && let Err(e) = self.inner.write_all(buf)
        {
            self.failed = Some(e);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.failed.is_none()
            && let Err(e) = self.inner.flush()
        {
            self.failed = Some(e);
        }
        Ok(())
    }
}

/// The result of a command that wrote through [`Unfailing`]: its own result, with the first
/// output error (`failed`, from each writer's [`Unfailing::into_error`]) added to an error or
/// turned into one.
pub fn after_output<const N: usize>(
    result: Result<()>,
    failed: [Option<std::io::Error>; N],
) -> Result<()> {
    let failed = failed.into_iter().flatten().next();
    match (result, failed) {
        (Ok(()), None) => Ok(()),
        (Ok(()), Some(io)) => Err(anyhow::Error::new(io)
            .context("the command ran to its end, but its output could not be written")),
        (Err(e), None) => Err(e),
        (Err(e), Some(io)) => Err(e.context(format!(
            "the command's output could not be written either ({io})"
        ))),
    }
}

/// A spec task's result under the `local` sink lives only in its `repo.git`, which removal deletes
/// (decision 174(e), spec section 5.4). Refused while its branch has commits beyond the base that
/// `checkout` (the user's git checkout) doesn't hold completely under a ref ([`holds_result`]), so
/// a result is never lost by accident; the caller skips this for `--force`. Only reads: `repo.git`
/// is host-owned (decision 159) and `checkout` is the user's own.
pub fn check_spec_result_fetched(base_dir: &Path, id: &str, checkout: &Path) -> Result<()> {
    let force = "or pass --force to delete it anyway";
    let meta = record::task_dir(base_dir, id);
    let repo_git = meta.join("repo.git");
    if !repo_git.is_dir() {
        return Ok(());
    }
    let task = record::read(&meta.join("task.json"))
        .with_context(|| format!("cannot tell whether task {id}'s result was fetched; {force}"))?;
    if !repo::valid_ref_name(&task.branch) {
        bail!(
            "task {id} records branch {:?}, which isn't a usable branch name; {force}",
            task.branch
        );
    }
    let Some(tip) = task.tip_if_ahead(&repo_git)? else {
        return Ok(());
    };
    check_result_held(id, &task, &repo_git, &tip, checkout)
}

/// [`check_spec_result_fetched`] for a caller that has read `task` and its branch's `tip` (which
/// has commits past its start) itself.
pub(crate) fn check_result_held(
    id: &str,
    task: &record::Record,
    repo_git: &Path,
    tip: &str,
    checkout: &Path,
) -> Result<()> {
    let force = "or pass --force to delete it anyway";
    let rev = task.rewrite_start();
    let start = repo::resolve_commits(repo_git, &[&rev])?
        .remove(0)
        .with_context(|| format!("{rev} doesn't resolve to a commit in the task repo"))?;
    if holds_result(checkout, &start, tip) {
        return Ok(());
    }
    let branch = &task.branch;
    bail!(
        "task {id}'s result, branch {branch}, exists only in {}, which `task rm` deletes; fetch it \
         first with: git fetch {} {branch}:{branch} (in your checkout), {force}",
        repo_git.display(),
        crate::commands::task_start::path_arg(repo_git)
    )
}

/// Whether `checkout` holds the result from `start` to `tip` safely (issue 324, gap G63): a ref
/// there points at `tip`, so `git gc` keeps it, and no object between them is missing, so a fetch
/// that failed partway after writing the tip commit doesn't pass. Anything git can't answer
/// counts as not held.
fn holds_result(checkout: &Path, start: &str, tip: &str) -> bool {
    let has_ref = crate::git::user_run(
        checkout,
        &["for-each-ref", "--points-at", tip, "--format=%(refname)"],
    )
    .is_ok_and(|refs| !refs.trim().is_empty());
    has_ref
        && crate::git::user_run(
            checkout,
            &[
                "rev-list",
                "--objects",
                "--missing=print",
                &format!("{start}..{tip}"),
            ],
        )
        .is_ok_and(|objects| !objects.lines().any(|line| line.starts_with('?')))
}

/// Shows what removing `id` would delete, asks (unless `yes`), removes it and says what went and
/// what stayed. Nothing is touched before the answer. Fails if anything stayed. `check` runs once
/// the task is planned, under its lock (issue 58), and before the question: an error refuses the
/// removal with nothing touched.
#[allow(clippy::too_many_arguments)]
pub fn discard(
    base_dir: &Path,
    id: &str,
    yes: bool,
    forget_ambiguous: bool,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    confirm: &dyn Confirm,
    out: &mut dyn Write,
    sleep: &dyn Fn(Duration),
    check: &dyn Fn() -> Result<()>,
) -> Result<()> {
    discard_many(
        base_dir,
        &[id.to_owned()],
        yes,
        forget_ambiguous,
        backend,
        probe,
        confirm,
        out,
        sleep,
        check,
    )
}

/// [`discard`] for several tasks at once (`start --restart --issue 1 --issue 2`): every task is
/// planned first, all of them are listed and confirmed with ONE question, and only then is the
/// first deleted, so a "no" (or a task that can't be planned) deletes nothing at all.
#[allow(clippy::too_many_arguments)]
pub fn discard_many(
    base_dir: &Path,
    ids: &[String],
    yes: bool,
    forget_ambiguous: bool,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    confirm: &dyn Confirm,
    out: &mut dyn Write,
    sleep: &dyn Fn(Duration),
    check: &dyn Fn() -> Result<()>,
) -> Result<()> {
    let plans = ids
        .iter()
        .map(|id| plan_removal_forgetting(base_dir, id, probe, forget_ambiguous))
        .collect::<Result<Vec<_>>>()?;
    check()?;
    remove_planned(&plans, &[], yes, backend, confirm, out, sleep)
}

/// [`discard_many`], where `check` is asked about each task once every one is planned, under its
/// lock (`task rm --pr`, issue 485): a task it refuses is kept, named with the reason before the
/// question and in the error at the end, and the others are still removed. Nothing is removed,
/// and nothing asked, when it refuses them all.
#[allow(clippy::too_many_arguments)]
pub fn discard_each(
    base_dir: &Path,
    ids: &[String],
    yes: bool,
    forget_ambiguous: bool,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    confirm: &dyn Confirm,
    out: &mut dyn Write,
    sleep: &dyn Fn(Duration),
    check: &dyn Fn(&str) -> Result<()>,
) -> Result<()> {
    let plans = ids
        .iter()
        .map(|id| plan_removal_forgetting(base_dir, id, probe, forget_ambiguous))
        .collect::<Result<Vec<_>>>()?;
    let mut kept = Vec::new();
    let mut removed = Vec::new();
    for plan in plans {
        match check(&plan.id) {
            Ok(()) => removed.push(plan),
            Err(e) => kept.push(format!("kept: task {}: {e:#}", plan.id)),
        }
    }
    if removed.is_empty() {
        bail!("nothing was removed:\n{}", kept.join("\n"));
    }
    remove_planned(&removed, &kept, yes, backend, confirm, out, sleep)?;
    if !kept.is_empty() {
        bail!(
            "{} task(s) kept, the others removed:\n{}",
            kept.len(),
            kept.join("\n")
        );
    }
    Ok(())
}

/// Lists `plans` (and the `kept` lines after them), asks (unless `yes`), removes them and says
/// what went and what stayed: the part [`discard_many`] and [`discard_each`] share.
fn remove_planned(
    plans: &[RemovalPlan],
    kept: &[String],
    yes: bool,
    backend: &dyn SandboxBackend,
    confirm: &dyn Confirm,
    out: &mut dyn Write,
    sleep: &dyn Fn(Duration),
) -> Result<()> {
    let ids: Vec<&str> = plans.iter().map(|p| p.id.as_str()).collect();
    let listing = plans
        .iter()
        .map(RemovalPlan::listing)
        .chain(kept.iter().cloned())
        .collect::<Vec<_>>()
        .join("\n");
    let names = ids.join(", ");
    let what = if ids.len() == 1 {
        format!("task {names}")
    } else {
        format!("{} tasks ({names})", ids.len())
    };
    if !yes {
        if !confirm.is_interactive() {
            bail!("refusing to remove {what} without a terminal; pass --yes to delete:\n{listing}");
        }
        if !confirm.confirm(&format!(
            "Permanently delete {what}: these sandboxes and folders?\n{listing}\n"
        ))? {
            bail!("removing {what} cancelled; nothing was deleted");
        }
    }
    let mut stayed = 0;
    for plan in plans {
        let report = remove_with_sleep(plan, backend, sleep);
        for line in &report.removed {
            writeln!(out, "removed {line}")?;
        }
        for line in &report.stayed {
            writeln!(out, "could not remove {line}")?;
        }
        stayed += report.stayed.len();
    }
    if stayed > 0 {
        bail!("{stayed} thing(s) of {what} stayed; fix that, then run `sbxm task rm` again");
    }
    Ok(())
}

/// The risk section at the very top of a PR description (decision 176(b, e)): the level the
/// review recorded (`Level::Unknown` for a record from before this existed) and its reasons, plus
/// a draft's own reasons (decision 176's "shows its must-fix reason in the same section"), so a
/// draft from a must-fix finding left shows why there, not only under `## Unresolved`. The draft's
/// reasons come first, so cutting a long reviewer reason to `risk::RISK_CAP` never drops them.
pub fn risk_section(task: &Record, unresolved: &[String]) -> String {
    let (level, reviewer_reasons) = match &task.review {
        Some(result) => (result.risk, result.risk_reasons.as_slice()),
        None => (risk::Level::default(), [].as_slice()),
    };
    let reasons: Vec<String> = unresolved.iter().chain(reviewer_reasons).cloned().collect();
    risk::render_section(level, &reasons)
}

/// The longest piece of `result.md` or `review.md` put in a PR body, in characters (GitHub allows
/// 65,536 for the whole body).
pub const SECTION_CAP: usize = 25_000;

/// The longest "Needs a decision" or "Choices made" section copied from `result.md` to the top
/// of a PR description, in characters (issue 220, decision 283). With the risk section's
/// `risk::RISK_CAP`, the draft's 5,000 and two [`SECTION_CAP`]s, the body stays under GitHub's
/// 65,536.
pub const HANDBACK_CAP: usize = 2_500;

/// The PR body: the [`risk_section`] first, then `Fixes #N`, the result's "Needs a decision" and
/// "Choices made" sections when they say something (issue 220, decision 283; each cut to
/// [`HANDBACK_CAP`]), the [`unresolved_section`] of a draft, and each file under a heading. A file
/// longer than the cap is cut and the body says so.
/// All of it but the `Fixes #N` line is agent text, so mentions and closing references in it are
/// neutralised ([`review::defang`]); the files in the task folder stay as they are. Returns the
/// body and one line per cut for the command to print.
pub fn pr_body(
    number: u32,
    risk: &str,
    unresolved: Option<&str>,
    result: Option<&str>,
    review: Option<&str>,
) -> (String, Vec<String>) {
    pr_body_fixing(&[number], risk, unresolved, result, review)
}

/// The line a PR body has in place of its `Fixes` lines when it closes no issue: a spec task
/// whose spec has no `Fixes:` line (issue 438, decision 301).
pub const CLOSES_NO_ISSUE: &str = "This PR closes no issue: its spec has no `Fixes:` line.";

/// [`pr_body`] with one `Fixes #n` line per number of `fixes`, in its order, or
/// [`CLOSES_NO_ISSUE`] when it is empty (a spec task's PR, issue 438).
pub fn pr_body_fixing(
    fixes: &[u32],
    risk: &str,
    unresolved: Option<&str>,
    result: Option<&str>,
    review: Option<&str>,
) -> (String, Vec<String>) {
    pr_body_referring(fixes, &[], risk, unresolved, result, review)
}

/// [`pr_body_fixing`] with a `Refs #n` line after the `Fixes` lines for each number of `refs`:
/// an issue the PR names but leaves open (`**Closes on merge:** no`, issue 506). A body with
/// only `refs` says nothing about closing no issue.
pub fn pr_body_referring(
    fixes: &[u32],
    refs: &[u32],
    risk: &str,
    unresolved: Option<&str>,
    result: Option<&str>,
    review: Option<&str>,
) -> (String, Vec<String>) {
    // Everything but the `Fixes #N` line is agent text, neutralised before any cut so a cut can't
    // separate a keyword from its zero-width space and leave the reference active. The risk
    // section is the same text the in-task review comment opens with (issue 228).
    let (risk, risk_cut) = review::published_risk(risk);
    let mut body = format!("{}\n", risk.trim_end());
    for number in fixes {
        body.push_str(&format!("Fixes #{number}\n"));
    }
    for number in refs {
        body.push_str(&format!("Refs #{number}\n"));
    }
    if fixes.is_empty() && refs.is_empty() {
        body.push_str(&format!("{CLOSES_NO_ISSUE}\n"));
    }
    let mut cuts: Vec<String> = risk_cut.into_iter().collect();
    for name in HANDBACK_SECTIONS {
        let Some(text) = result.and_then(|r| handback_section(r, name)) else {
            continue;
        };
        let text = defang(&text);
        let total = text.chars().count();
        body.push_str(&format!("\n## {name}\n\n"));
        if total <= HANDBACK_CAP {
            body.push_str(text.trim_end());
            body.push('\n');
        } else {
            let head: String = text.chars().take(HANDBACK_CAP).collect();
            body.push_str(head.trim_end());
            body.push_str(&format!(
                "\n\n(cut: the \"{name}\" section has {total} characters; the first {HANDBACK_CAP} are shown, the whole of it is in result.md in the task folder)\n"
            ));
            cuts.push(format!(
                "result.md's \"{name}\" section is {total} characters; the PR body's copy carries the first {HANDBACK_CAP}"
            ));
        }
    }
    if let Some(section) = unresolved {
        body.push_str(&format!("\n{}\n", defang(section).trim_end()));
    }
    for (file, heading, text) in [
        ("result.md", "Result", result),
        ("review.md", "Review", review),
    ] {
        body.push_str(&format!("\n## {heading}\n\n"));
        let Some(text) = text else {
            body.push_str(&format!("(no {file})\n"));
            continue;
        };
        let text = &defang(text);
        let total = text.chars().count();
        if total <= SECTION_CAP {
            body.push_str(text.trim_end());
            body.push('\n');
        } else {
            let head: String = text.chars().take(SECTION_CAP).collect();
            body.push_str(head.trim_end());
            body.push_str(&format!(
                "\n\n(cut: {file} has {total} characters; the first {SECTION_CAP} are shown, the rest is in the task folder)\n"
            ));
            cuts.push(format!(
                "{file} is {total} characters; the PR body carries the first {SECTION_CAP}"
            ));
        }
    }
    (body, cuts)
}

/// The two sections of a worker's `result.md` copied to the top of a PR description, in their
/// order there (issue 220, decision 283).
const HANDBACK_SECTIONS: [&str; 2] = ["Needs a decision", "Choices made"];

/// Whether `result` (a worker's `result.md`) hands a question back: its "Needs a decision"
/// section has something in it (issue 448).
pub fn hands_back(result: &str) -> bool {
    handback_section(result, "Needs a decision").is_some()
}

/// The text of `result`'s section called `name` (issue 220, decision 283): under a heading of any
/// level whose text is `name` in any case, with bold or italic marks and a trailing colon allowed
/// (`## Needs a decision: none` starts its text with `none`), up to the next heading at its level
/// or above; a heading inside a code fence neither starts nor ends a section. Several such
/// sections are joined. `None` when there is none, or all of it is empty or "none".
fn handback_section(result: &str, name: &str) -> Option<String> {
    let mut parts = Vec::new();
    let mut current: Option<(usize, Vec<&str>)> = None;
    let mut fence: Option<(char, usize)> = None;
    for line in result.lines() {
        let heading = match fence {
            None => heading_of(line),
            Some(_) => None,
        };
        fence = next_fence(fence, line);
        if let Some((level, text)) = heading {
            if current.as_ref().is_some_and(|(at, _)| level <= *at) {
                parts.extend(current.take().map(|(_, lines)| lines.join("\n")));
            }
            if current.is_none() {
                current = named_text(text, name).map(|inline| (level, vec![inline]));
                if current.is_some() {
                    continue;
                }
            }
        }
        if let Some((_, lines)) = &mut current {
            lines.push(line);
        }
    }
    parts.extend(current.map(|(_, lines)| lines.join("\n")));
    let parts: Vec<&str> = parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !is_none(p))
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// An ATX heading's level and text (`### Text ##` is level 3, `Text`).
fn heading_of(line: &str) -> Option<(usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let level = rest.len() - rest.trim_start_matches('#').len();
    let text = &rest[level..];
    if !(1..=6).contains(&level) || !(text.is_empty() || text.starts_with([' ', '\t'])) {
        return None;
    }
    Some((level, text.trim().trim_end_matches('#').trim()))
}

/// The heading `text` without its bold and italic marks, when it names `name`: the rest of the
/// text after a colon (empty without one), or `None` for another heading.
fn named_text<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let plain = text.trim_matches(['*', '_', ' ']);
    let start = plain.get(..name.len())?;
    if !start.eq_ignore_ascii_case(name) {
        return None;
    }
    let rest = plain[name.len()..].trim_start_matches(['*', '_']);
    match rest.strip_prefix(':') {
        Some(inline) => Some(inline.trim_matches(['*', '_', ' '])),
        None => rest.is_empty().then_some(""),
    }
}

/// The code fence open after `line`, given the one open before it (a run of three or more
/// backticks or tildes opens one; the same character, at least as many, alone closes it).
pub(crate) fn next_fence(open: Option<(char, usize)>, line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return open;
    }
    let marker = ['`', '~'].into_iter().find_map(|c| {
        let run = trimmed.len() - trimmed.trim_start_matches(c).len();
        (run >= 3).then_some((c, run))
    });
    match (open, marker) {
        (None, Some(found)) => Some(found),
        (Some((c, n)), Some((m, run)))
            if c == m && run >= n && trimmed[run..].trim().is_empty() =>
        {
            None
        }
        (open, _) => open,
    }
}

/// Whether a section's text says nothing: empty, or "none" give or take list, bold and italic
/// marks and a full stop.
fn is_none(text: &str) -> bool {
    let plain = text.trim_matches(|c: char| c.is_whitespace() || "-*_+.".contains(c));
    plain.is_empty() || plain.eq_ignore_ascii_case("none")
}

/// Why task `task`'s PR must be a draft (decision 173(b)), one line per reason; empty when it is
/// clean. The must-fix count is the review result recorded in `task.json` (decision 177(f)), or,
/// for a record from before it was kept, the count in the saved `review`. A count that can't be
/// read is a reason too: a task with must-fix findings left never gets a normal PR.
pub fn unresolved_reasons(task: &Record, review: Option<&str>) -> Vec<String> {
    let mut reasons = Vec::new();
    if let Some(stopped) = task.stopped {
        let why = match stopped {
            record::Stopped::RoundsExhausted => "it used all its fix rounds",
            record::Stopped::RepeatFinding => "a must-fix finding came back after a fix round",
        };
        reasons.push(format!("the task stopped ({}): {why}", stopped.name()));
    }
    let must_fix = match &task.review {
        Some(recorded) => Some(recorded.must_fix),
        None => review.and_then(super::review::saved_must_fix_count),
    };
    match (must_fix, review) {
        (Some(0), _) => {}
        (Some(n), _) => reasons.push(format!("the last review has {n} must-fix finding(s) left")),
        (None, Some(_)) => reasons.push("review.md has no must-fix count".to_owned()),
        (None, None) => reasons.push("no review result is recorded".to_owned()),
    }
    reasons
}

/// The longest list of findings the draft's section carries, in characters; the review itself
/// follows in full (up to [`SECTION_CAP`]).
const UNRESOLVED_CAP: usize = 5_000;

/// Ends a finding cut to fit [`UNRESOLVED_CAP`].
const CUT_MARK: &str = "… (cut)\n";

/// The must-fix findings task `task` left open: those recorded in `task.json` when its reviews
/// were accepted (decision 177(f, p)), so an edited `review.md` changes nothing. A record from
/// before they were kept rebuilds them from the `review-<round>.md` files saved in its folder
/// `meta`, like the next review would; with none of those, they are the saved `review`'s.
pub fn open_findings(task: &Record, meta: &Path, review: Option<&str>) -> Vec<OpenFinding> {
    if let Some(open) = &task.open_findings {
        return open.clone();
    }
    let rebuilt = review::open_from_task_dir(meta, u32::MAX);
    if !rebuilt.is_empty() {
        return rebuilt;
    }
    review
        .map(|r| review::must_fix_findings(r, 0))
        .unwrap_or_default()
}

/// Refuses a secret-looking open finding before any of it is cut to fit a PR's text, naming the
/// finding by its place in the list, never the value.
fn refuse_secret_findings(open: &[OpenFinding]) -> Result<()> {
    for (n, finding) in open.iter().enumerate() {
        let place = finding.place.as_deref().unwrap_or_default();
        for text in [finding.id.as_str(), finding.title.as_str(), place] {
            if let Some(kind) = text.lines().find_map(super::findings::secret_kind) {
                bail!(
                    "the draft's list of open findings: finding {} of {} looks like a secret \
                     ({kind}); remove it from the task folder's task.json or saved reviews, then \
                     run `sbxm task finish` again",
                    n + 1,
                    open.len()
                );
            }
        }
    }
    Ok(())
}

/// The section at the top of a draft PR's body: why it is a draft, then each must-fix finding
/// left open ([`open_findings`]) with its `Where:`.
pub fn unresolved_section(reasons: &[String], open: &[OpenFinding]) -> String {
    unresolved_text("This PR is a draft because:", reasons, open)
}

/// [`unresolved_section`] under `intro`: a continued PR isn't made a draft (decision 177(r)).
fn unresolved_text(intro: &str, reasons: &[String], must_fix: &[OpenFinding]) -> String {
    let mut text = format!("## Unresolved\n\n{intro}\n\n");
    for reason in reasons {
        text.push_str(&format!("- {reason}\n"));
    }
    if must_fix.is_empty() {
        return text;
    }
    text.push_str("\nMust-fix findings left (the review below has the details):\n\n");
    let mut listed = 0;
    for (i, finding) in must_fix.iter().enumerate() {
        // Ids restart with each review, so a recorded finding names the round that found it.
        let id = match finding.round {
            0 => finding.id.clone(),
            round => format!("{} (review {round})", finding.id),
        };
        let line = match &finding.place {
            Some(place) => format!("- {id}: {} ({place})\n", finding.title),
            None => format!("- {id}: {}\n", finding.title),
        };
        // A finding is listed only if the `and N more` line for the ones after it still fits,
        // so the section never passes the cap whichever finding is the first left out.
        let after = more_line(must_fix.len() - i - 1).chars().count();
        if text.chars().count() + line.chars().count() + after > UNRESOLVED_CAP {
            if listed == 0 {
                // A first finding too long on its own is cut instead of left out.
                let room = UNRESOLVED_CAP
                    .saturating_sub(text.chars().count() + CUT_MARK.chars().count() + after);
                text.extend(line.chars().take(room));
                text.push_str(CUT_MARK);
                listed = 1;
            }
            break;
        }
        text.push_str(&line);
        listed += 1;
    }
    text.push_str(&more_line(must_fix.len() - listed));
    text
}

/// The line counting `left` findings not listed; empty when there are none.
fn more_line(left: usize) -> String {
    if left == 0 {
        String::new()
    } else {
        format!("- and {left} more\n")
    }
}

/// The note `finish` leaves on a task whose branch is pushed but whose PR isn't open yet.
const PUSHED_NOTE: &str =
    "the branch is pushed but its PR isn't open; run `sbxm task finish` again";

/// What `finish` did.
#[derive(Debug)]
pub struct Finished {
    pub url: String,
    /// The branch that was pushed.
    pub branch: String,
    /// Things cut from the PR body.
    pub cuts: Vec<String>,
    /// The PR the task pushed to, when it continues one (decision 169): no PR was opened.
    pub continued: Option<u32>,
    /// The PR was already open for the branch and base, so it was recorded instead of opened
    /// (issue 60): a create whose answer was lost, or one opened by hand.
    pub adopted: bool,
    /// Whether the PR was opened as a draft, and why ([`unresolved_reasons`]).
    pub draft: bool,
    pub unresolved: Vec<String>,
    /// The commit whose in-task review was posted on the new PR as its review (issue 198);
    /// `None` when no recorded review covered the PR's head.
    pub review_posted: Option<String>,
    /// What went wrong after the PR was opened without failing the finish: the `sbxm/review`
    /// status that couldn't be posted (issue 317).
    pub warnings: Vec<String>,
    /// The `sbxm task review --pr` command for the PR's next commit
    /// ([`review::review_pr_command`]).
    pub review_again: String,
    /// The task was already finished with its PR, so this finish only posted or edited the
    /// summaries on `summarized` (issue 430): nothing was pushed, opened or reviewed.
    pub summaries_only: bool,
    /// The issues whose summary this finish posted or edited, in order (failures are in
    /// `warnings`).
    pub summarized: Vec<u32>,
    /// The issues the PR's description refers to with `Refs #N` and leaves open, as their body's
    /// `**Closes on merge:** no` asks (issue 506); empty for a PR that fixes its issue.
    pub left_open: Vec<u32>,
}

/// The number at the end of a PR's URL (`https://github.com/o/r/pull/7`), as `gh pr create`
/// prints it and `task.json` records it.
pub fn pr_number(url: &str) -> Option<u32> {
    let (_, number) = url.trim().trim_end_matches('/').rsplit_once("/pull/")?;
    number.parse().ok()
}

/// The local issue task (or spec task under the `pr` sink, issue 438) that opened PR `number` of
/// `repo` with `task finish`, and the commit its in-task review covered (issue 198), read only
/// from the task folders on this machine. A task that continued the PR doesn't count: its review
/// covered only its own commits, not the PR.
/// Only `review.posted` counts, which `task finish` records once GitHub accepted the comment (PR
/// 226's review): a task that had no `review.md` to post, whose post failed, or that was
/// interrupted before the post has none, so its PR is reviewed as usual. A folder whose
/// `task.json` can't be read is skipped: at worst the PR is reviewed again.
pub fn reviewed_pr_task(base_dir: &Path, repo: &str, number: u32) -> Option<(String, String)> {
    let url = format!("https://github.com/{repo}/pull/{number}");
    let entries = fs::read_dir(record::tasks_root(base_dir)).ok()?;
    let mut found: Vec<(String, String)> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("issue-") || name.starts_with("spec-")
        })
        .filter_map(|e| record::read(&e.path().join("task.json")).ok())
        .filter(|t| matches!(t.kind, record::Kind::Issue | record::Kind::Spec))
        .filter(|t| t.continues.is_none())
        .filter(|t| {
            t.pr.as_deref()
                .is_some_and(|pr| pr.trim().eq_ignore_ascii_case(&url))
        })
        .filter_map(|t| Some((t.id, t.review?.posted?)))
        .collect();
    found.sort();
    found.into_iter().next()
}

/// The commit task `task`'s recorded review covered, when that is `tip`, the head the PR will
/// have (issue 198): only then does the in-task review count as the PR's review.
fn reviewed_head(task: &Record, tip: &str) -> Option<String> {
    task.review
        .as_ref()
        .and_then(|r| r.commit.clone())
        .filter(|commit| commit == tip)
}

/// Whether `task finish` on this task is the summary-only retry of issue 430: a finished issue or
/// spec task with a recorded PR. Decided through `machine::TABLE` once the `pr` field is checked;
/// a spec task under the `local` or `push` sink records no PR, and a fix task is never a PR of
/// its own.
pub fn check_can_retry_summaries(task: &Record) -> Result<()> {
    let id = &task.id;
    if task.pr.is_none() {
        bail!("task {id} has no recorded PR, so there are no summaries to retry");
    }
    let state = machine::State {
        stage: task.stage,
        status: task.status,
        interrupted: false,
        kind: task.kind,
    };
    if machine::verdict(&state, machine::Event::FinishSummaries) {
        Ok(())
    } else {
        bail!(
            "task {id} ({:?} task, stage {}) has no summaries to retry",
            task.kind,
            task.stage.name()
        )
    }
}

/// Whether the task may be finished now. Decided through `machine::TABLE` once the `pr` field (not
/// part of a table state; see `sdlc/spikes/state-table.md`) is checked. The reason for a refusal
/// says what to do.
pub fn check_can_finish(task: &Record) -> Result<()> {
    let (id, number) = (&task.id, task.number);
    if let Some(url) = &task.pr {
        bail!("task {id} already has a PR, {url}; there is nothing left to finish");
    }
    let state = machine::State {
        stage: task.stage,
        status: task.status,
        // No command retries an interrupted task towards `finish` yet, so this check never sees
        // one; `finish` itself only ever runs on `ready`, never on a `running` status.
        interrupted: false,
        kind: task.kind,
    };
    if machine::verdict(&state, machine::Event::Finish) {
        return Ok(());
    }
    match task.kind {
        record::Kind::Pr => bail!("task {id} is a PR review; only an issue's task can be finished"),
        record::Kind::Issue | record::Kind::Spec | record::Kind::Fix => {}
    }
    let spec = task.kind == record::Kind::Spec;
    match (task.stage, task.status) {
        (record::Stage::Finished, _) if spec => {
            bail!(
                "task {id} is already finished; its branch {} is in its repo.git",
                task.branch
            )
        }
        (record::Stage::Finished, _) => {
            bail!("task {id} is already finished; its PR is recorded in task.json")
        }
        (stage, status) => bail!(
            "task {id} is at stage {} ({}), not ready; run `sbxm task review {}` first",
            stage.name(),
            status.name(),
            if spec {
                "--spec <FILE>".to_owned()
            } else {
                format!("--issue {number}")
            }
        ),
    }
}

fn read_capped(path: &Path) -> Result<Option<String>> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Ok(None);
    };
    if !meta.file_type().is_file() {
        bail!(
            "{} isn't a plain file; refusing to publish it",
            path.display()
        );
    }
    let bytes = fs::read(path)?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// Whether `path` is part of the repository's CI: GitHub runs what is in `.github/workflows/` and
/// `.github/actions/`, with the repository's secrets, as soon as a branch is pushed. Compared
/// without regard to letter case, so `.GitHub/Workflows/` can't slip past.
fn is_ci_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with(".github/workflows/") || lower.starts_with(".github/actions/")
}

/// The agent's commits are untrusted (decision 127): a same-repo branch that adds or edits a
/// workflow would run it, with secrets, on the push, before anyone has read it. So `finish`
/// refuses, naming the files, and the user can review and push the branch by hand if it is meant.
fn refuse_workflow_changes(id: &str, changed: &[String]) -> Result<()> {
    let ci: Vec<&str> = changed
        .iter()
        .map(String::as_str)
        .filter(|p| is_ci_path(p))
        .collect();
    if ci.is_empty() {
        return Ok(());
    }
    bail!(
        "task {id} changes {}, which GitHub would run with this repository's secrets as soon as \
         the branch is pushed; read those files in the task's repo.git, and push the branch \
         yourself if the change is meant",
        ci.join(", ")
    );
}

/// `<name> line <n> looks like a secret (<kind>)` for the first line of `text` that does.
fn secret_line(name: &str, text: &str) -> Option<String> {
    text.lines().enumerate().find_map(|(n, line)| {
        super::findings::secret_kind(line)
            .map(|kind| format!("{name} line {} looks like a secret ({kind})", n + 1))
    })
}

fn refuse_secrets(name: &str, text: &str) -> Result<()> {
    match secret_line(name, text) {
        Some(found) => bail!(
            "{found}; remove it from the file in the task folder, then run `sbxm task finish` again"
        ),
        None => Ok(()),
    }
}

/// Pushes the task's branch from its `repo.git` and opens the PR (spec §4): every check first,
/// then the push, then the PR, then the record. A PR that can't be opened after the push leaves
/// the task `ready` with a note, so running `finish` again only retries the PR (the push of the
/// same commits is then a no-op, and it never forces).
pub fn finish(
    base_dir: &Path,
    id: &str,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
) -> Result<Finished> {
    open_task_pr(base_dir, id, None, github, probe)
}

/// The `pr` sink for a spec task (issue 438, decision 301): [`finish`], as for an issue task,
/// but the PR closes the issues of the spec's `Fixes:` line, read from the copy its start saved
/// (`source.md`) before anything is pushed, and its title is the spec's first heading. `retry` is
/// the `--spec <FILE>` the refusals name.
pub fn finish_spec_pr(
    base_dir: &Path,
    id: &str,
    retry: &str,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
) -> Result<Finished> {
    open_task_pr(base_dir, id, Some(retry), github, probe)
}

/// [`finish`] for an issue task (`spec` is `None`), or [`finish_spec_pr`] for a spec task, whose
/// `--spec <FILE>` flag `spec` is.
fn open_task_pr(
    base_dir: &Path,
    id: &str,
    spec: Option<&str>,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
) -> Result<Finished> {
    let mut prepared = Prepared::open(base_dir, id)?;
    let is_spec = prepared.record.kind == record::Kind::Spec;
    if is_spec && spec.is_none() {
        bail!("task {id} is a spec task; it finishes through its [finish] sink, never a PR");
    }
    if !is_spec && spec.is_some() {
        bail!("task {id} isn't a spec task; only a spec task has a [finish] sink");
    }
    if prepared.record.kind == record::Kind::Fix {
        bail!("task {id} is a fix task; `sbxm task fix` pushes it onto its PR's branch itself");
    }
    if check_can_retry_summaries(&prepared.record).is_ok() {
        return retry_summaries(prepared, github);
    }
    check_can_finish(&prepared.record)?;
    let task = prepared.record.clone();
    crate::commands::task_start::check_repo(&task.repo)?;
    for (what, name) in [("branch", &task.branch), ("base", &task.base)] {
        if !repo::valid_ref_name(name) {
            bail!(
                "task {id} records {what} {name:?}, which isn't a usable branch name; see task.json"
            );
        }
    }
    let repo_git = prepared.meta.join("repo.git");
    if !repo_git.is_dir() {
        bail!("task {id} has no repo.git, so there is nothing to push; see `sbxm task status`");
    }
    let Some(tip) = task.tip_if_ahead(&repo_git)? else {
        let from = task.continues.as_ref().map_or(&task.base, |c| &c.base);
        bail!(
            "task {id} has no commits on {} beyond {from}, so there is nothing to publish",
            task.branch,
        );
    };
    refuse_workflow_changes(id, &task.changed_paths(&repo_git)?)?;
    if let Some(continued) = &task.continues {
        return finish_continued(prepared, &repo_git, continued, &tip, github, probe);
    }
    // What the PR closes, its title, and the flag a retry names: the issue's, or the spec's.
    // An issue whose body says `**Closes on merge:** no` is referred to, not fixed (issue 506).
    let (fixes, refs, title, retry) = match spec {
        None => {
            let issue = vec![task.number];
            let (fixes, refs) = if issue_leaves_open(&prepared)? {
                (Vec::new(), issue)
            } else {
                (issue, Vec::new())
            };
            (
                fixes,
                refs,
                task.title.clone(),
                format!("--issue {}", task.number),
            )
        }
        Some(flag) => {
            let (fixes, text) = saved_fixes(&prepared)?;
            (
                fixes,
                Vec::new(),
                super::fixes::title(&text, &task.title),
                flag.to_owned(),
            )
        }
    };
    let result = read_capped(&prepared.meta.join("result.md"))?;
    let review = read_capped(&prepared.meta.join("review.md"))?;
    for (name, text) in [("result.md", &result), ("review.md", &review)] {
        if let Some(text) = text {
            refuse_secrets(name, text)?;
        }
    }
    let unresolved = unresolved_reasons(&task, review.as_deref());
    let draft = !unresolved.is_empty();
    let open = if draft {
        open_findings(&task, &prepared.meta, review.as_deref())
    } else {
        Vec::new()
    };
    refuse_secret_findings(&open)?;
    let section = draft.then(|| unresolved_section(&unresolved, &open));
    if let Some(section) = &section {
        // Its findings come from task.json, which no file check above covered.
        refuse_secrets("the draft's list of open findings", section)?;
    }
    let risk_text = risk_section(&task, &unresolved);
    refuse_secrets("the risk section", &risk_text)?;
    let (body, cuts) = pr_body_referring(
        &fixes,
        &refs,
        &risk_text,
        section.as_deref(),
        result.as_deref(),
        review.as_deref(),
    );
    // Issue 198: the in-task review becomes the PR's review when it covered the head pushed now.
    let posting = reviewed_head(&task, &tip).zip(review.clone());

    // Issue 60: an open PR of this branch is looked for before anything is pushed. One into the
    // task's base is adopted rather than opened again; one into another base is named and left
    // alone, since pushing would move its head.
    let existing = open_pr(github, &task, &retry)?;
    if let Some(Found::OtherBase(pr)) = &existing {
        bail!(
            "{} is already open for branch {} but into {}, not {}; nothing was pushed and no \
             second PR was opened; close or retarget that PR, then run `sbxm task finish {retry}` \
             again",
            pr.url,
            task.branch,
            pr.base_ref,
            task.base,
        );
    }
    repo::push(&repo_git, &task.branch)?;
    let request = PrRequest {
        head: task.branch.clone(),
        base: task.base.clone(),
        title,
        body,
        draft,
    };
    let (url, adopted) = match existing {
        Some(Found::Matching(pr)) => (pr.url, true),
        _ => match github.pr_create(&task.repo, &request) {
            Ok(url) => (url, false),
            // A failed create is decided by reading the PR state again, never by its error
            // text (the lesson of #319): GitHub may have opened the PR and lost only its answer.
            Err(e) => match open_pr(github, &task, &retry) {
                Ok(Some(Found::Matching(pr))) => (pr.url, true),
                _ => {
                    if !prepared.record.notes.iter().any(|n| n == PUSHED_NOTE) {
                        prepared.record.notes.push(PUSHED_NOTE.to_owned());
                        let _ = record::write(&prepared.meta, &prepared.record);
                    }
                    return Err(e).context(format!(
                        "pushed {} but could not open the PR; run `sbxm task finish {retry}` again, which only retries the PR",
                        task.branch
                    ));
                }
            },
        },
    };
    prepared.record.notes.retain(|n| n != PUSHED_NOTE);
    prepared
        .record
        .advance(record::Stage::Finished, now(), Process::current(probe))?;
    prepared.record.pr = Some(url.clone());
    record::write(&prepared.meta, &prepared.record).with_context(|| {
        format!("the PR is open at {url} but task.json couldn't be updated; note the URL")
    })?;
    let pr = pr_number(&url).map_or_else(|| "<N>".to_owned(), |n| n.to_string());
    // Issue 430: before the in-task review, whose failed post fails the finish.
    let issues: Vec<u32> = fixes.iter().chain(&refs).copied().collect();
    let (summarized, mut warnings) = post_issue_summaries(&mut prepared, &issues, &url, github);
    let review_posted = match posting {
        Some((commit, text)) => {
            let (must_fix, level) = task
                .review
                .as_ref()
                .map_or((0, risk::Level::default()), |r| (r.must_fix, r.risk));
            let reviewed = super::pipeline::StatusInput {
                commit: Some(&commit),
                must_fix,
                text: &text,
                risk: level,
            };
            warnings.extend(post_in_task_review(
                &mut prepared,
                &url,
                &reviewed,
                &risk_text,
                github,
            )?);
            Some(commit)
        }
        None => None,
    };
    Ok(Finished {
        review_again: review::review_pr_command(pr, &task.repo, Some(&task.base), false),
        url,
        branch: task.branch,
        cuts,
        continued: None,
        adopted,
        draft,
        unresolved,
        review_posted,
        warnings,
        summaries_only: false,
        summarized,
        left_open: refs,
    })
}

/// Whether the issue text an issue task's start saved (`issue.md`) asks for the PR to leave the
/// issue open ([`super::closes::leaves_open`], issue 506). A task folder without it closes the
/// issue, as before; one that can't be read is refused before anything is pushed.
fn issue_leaves_open(prepared: &Prepared) -> Result<bool> {
    let path = prepared.meta.join("issue.md");
    match fs::read_to_string(&path) {
        Ok(text) => Ok(super::closes::leaves_open(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| {
            format!(
                "cannot read {}, the issue as the task's start saved it, to see whether the PR \
                 closes it; nothing was pushed",
                path.display()
            )
        }),
    }
}

/// The issues a spec task's PR closes, from the `Fixes:` line of the spec as its start saved it
/// (`source.md`), and that saved text.
fn saved_fixes(prepared: &Prepared) -> Result<(Vec<u32>, String)> {
    let id = &prepared.record.id;
    let text = fs::read_to_string(prepared.meta.join("source.md")).with_context(|| {
        format!(
            "task {id} has no readable source.md, the spec as its start saved it, so its Fixes \
             line can't be read; nothing was pushed"
        )
    })?;
    let fixes = super::fixes::parse(&text).with_context(|| {
        format!("task {id}'s spec, as its start saved it in source.md; nothing was pushed")
    })?;
    Ok((fixes, text))
}

/// A finish of a task already finished with its PR (issue 430): it only posts the summary of the
/// review rounds on each issue the PR closes, or edits the comment `task.json` recorded for it,
/// so a summary whose post failed can be retried and a recorded one updated. The PR comes from
/// `task.json`, the issues from the task's number or its saved spec, both read before any call;
/// nothing is pushed, no PR is looked up or opened, and no review or status is posted again.
fn retry_summaries(mut prepared: Prepared, github: &dyn GitHubBackend) -> Result<Finished> {
    let task = prepared.record.clone();
    crate::commands::task_start::check_repo(&task.repo)?;
    let url = task.pr.clone().unwrap_or_default();
    let issues = match task.kind {
        record::Kind::Spec => saved_fixes(&prepared)?.0,
        _ => vec![task.number],
    };
    let (summarized, warnings) = post_issue_summaries(&mut prepared, &issues, &url, github);
    let pr = pr_number(&url).map_or_else(|| "<N>".to_owned(), |n| n.to_string());
    let continued = task.continues.as_ref().map(|c| c.pr);
    let base = continued.is_none().then_some(task.base.as_str());
    Ok(Finished {
        review_again: review::review_pr_command(pr, &task.repo, base, false),
        url,
        branch: task.branch,
        cuts: Vec::new(),
        continued,
        adopted: false,
        draft: false,
        unresolved: Vec::new(),
        review_posted: None,
        warnings,
        summaries_only: true,
        summarized,
        left_open: Vec::new(),
    })
}

/// Posts the [`summary::issue_comment`] on each of `issues`, in order (issue 430), or edits the
/// one `task.json` recorded for that issue: only the task's own record says which comment is its,
/// never a marker in the issue's comments (decision 187(d)). A comment's id is recorded once
/// GitHub accepted it. Nothing here fails the finish: every failure is a warning returned, and
/// the other issues still get theirs. Returns the issues whose comment GitHub accepted, in
/// order, and the warnings.
fn post_issue_summaries(
    prepared: &mut Prepared,
    issues: &[u32],
    url: &str,
    github: &dyn GitHubBackend,
) -> (Vec<u32>, Vec<String>) {
    let (mut done, mut warnings) = (Vec::new(), Vec::new());
    if issues.is_empty() {
        return (done, warnings);
    }
    let body = match summary::issue_comment(&prepared.record, &prepared.meta, url) {
        Ok(body) => body,
        Err(why) => {
            warnings.push(format!(
                "no summary of the review rounds was posted on the issue: {why}"
            ));
            return (done, warnings);
        }
    };
    let repo = prepared.record.repo.clone();
    for &issue in issues {
        let recorded = prepared
            .record
            .issue_comments
            .iter()
            .find(|c| c.issue == issue)
            .map(|c| c.id);
        match recorded {
            Some(id) => match github.issue_comment_edit(&repo, id, &body) {
                Ok(()) => done.push(issue),
                Err(e) => warnings.push(format!(
                    "the summary of the review rounds on issue #{issue} (comment {id}) \
                         couldn't be updated: {e:#}"
                )),
            },
            None => match github.issue_comment(&repo, issue, &body) {
                Ok(id) => {
                    done.push(issue);
                    prepared
                        .record
                        .issue_comments
                        .push(record::IssueComment { issue, id });
                    if let Err(e) = record::write(&prepared.meta, &prepared.record) {
                        warnings.push(format!(
                            "the summary of the review rounds is posted on issue #{issue}, but \
                             task.json couldn't record it ({e:#}); a later finish posts another"
                        ));
                    }
                }
                Err(e) => warnings.push(format!(
                    "the summary of the review rounds couldn't be posted on issue #{issue}: {e:#}"
                )),
            },
        }
    }
    (done, warnings)
}

/// What [`open_pr`] found for a task's branch.
enum Found {
    /// Open, of the task's repo, head and base: the task's PR.
    Matching(ListedPr),
    /// Open, of the task's repo and head, into another base.
    OtherBase(ListedPr),
}

/// The open PR of `task`'s branch, by repo, head and base (issue 60); a failed lookup says which
/// call failed.
fn open_pr(
    github: &dyn GitHubBackend,
    task: &record::Record,
    retry: &str,
) -> Result<Option<Found>> {
    let found = github
        .open_pr_for(&task.repo, &task.branch, &task.base)
        .with_context(|| {
            format!(
                "could not look up an open PR of branch {}; nothing was opened; run `sbxm task \
                 finish {retry}` again",
                task.branch
            )
        })?;
    Ok(found
        .filter(|pr| pr.head_ref == task.branch && !pr.is_cross_repository)
        .map(|pr| {
            if pr.base_ref == task.base {
                Found::Matching(pr)
            } else {
                Found::OtherBase(pr)
            }
        }))
}

/// Posts the in-task review of `commit` on the PR at `url` (issue 198), with `risk_text`, the
/// section the PR description opens with, first, and then records the post as `review.posted`,
/// the only thing that makes `task review --pr` treat the head as reviewed (PR 226's review). The
/// PR is already open and recorded, so a post that fails, or one that can't be recorded, leaves
/// an error naming the review to run instead. It names no command that posts `review.md`: that
/// file has neither the risk section nor the commit.
fn post_in_task_review(
    prepared: &mut Prepared,
    url: &str,
    reviewed: &super::pipeline::StatusInput,
    risk_text: &str,
    github: &dyn GitHubBackend,
) -> Result<Vec<String>> {
    let commit = reviewed.commit.unwrap_or_default();
    let task = &prepared.record;
    let review_pr =
        |pr: &str, force: bool| review::review_pr_command(pr, &task.repo, Some(&task.base), force);
    let mut warnings = Vec::new();
    let posted = match pr_number(url) {
        Some(pr) => {
            let body = match task.kind {
                record::Kind::Spec => super::review::in_task_spec_pr_comment(
                    &task.id,
                    commit,
                    risk_text,
                    reviewed.text,
                ),
                _ => {
                    super::review::in_task_pr_comment(task.number, commit, risk_text, reviewed.text)
                }
            };
            // Issue 317: the review is the PR head's now, so only `--force` reviews it again.
            let again = review_pr(&pr.to_string(), true);
            let published = review::publish(github, &task.repo, pr, &body, reviewed, &again);
            warnings = published.warnings;
            published.comment.map(|_| pr).map_err(|e| (Some(pr), e))
        }
        None => Err((None, anyhow::anyhow!("{url} doesn't end in a PR number"))),
    };
    let (pr, e) = match posted {
        Ok(pr) => {
            let hint = review_pr(&pr.to_string(), false);
            if let Some(review) = &mut prepared.record.review {
                review.posted = Some(commit.to_owned());
            }
            return record::write(&prepared.meta, &prepared.record)
                .with_context(|| {
                    format!(
                        "the in-task review is posted on {url}, but task.json couldn't be updated \
                         to say so; `{hint}` will review the PR again"
                    )
                })
                .map(|()| warnings);
        }
        Err(failed) => failed,
    };
    let pr = pr.map_or_else(|| "<N>".to_owned(), |n| n.to_string());
    let hint = review_pr(&pr, false);
    prepared.record.notes.push(format!(
        "the in-task review was not posted on PR #{pr}: {e:#}"
    ));
    let _ = record::write(&prepared.meta, &prepared.record);
    Err(e.context(format!(
        "the PR is open at {url}, but the in-task review couldn't be posted on it; review the PR \
         with `{hint}`"
    )))
}

/// What `task fix --pr` did after its review (issue 313): its commits are on the PR's branch.
#[derive(Debug)]
pub struct FixPushed {
    pub url: String,
    pub branch: String,
    /// The PR's head now: the fix task's tip.
    pub commit: String,
    /// The fix task's own commits, `<short id> <subject>`, oldest first.
    pub commits: Vec<String>,
    /// Why the review still has something unresolved ([`unresolved_reasons`]); empty when clean.
    pub unresolved: Vec<String>,
    /// Whether the in-task review was posted as the review of [`commit`](Self::commit); `false`
    /// when no recorded review covered it.
    pub review_posted: bool,
    /// The `sbxm task review --pr` command for the PR's head.
    pub review_again: String,
    /// Why the `sbxm/review` status wasn't posted on the new head, when it wasn't (issue 469).
    pub warnings: Vec<String>,
}

/// The line that says where a fix task's result stays when it can't be pushed (issue 313,
/// decision 3 of the issue): the task's `repo.git` and the branch there.
fn kept_in(prepared: &Prepared, branch: &str) -> String {
    format!(
        "the result is kept in {} as {branch}",
        prepared.meta.join("repo.git").display()
    )
}

/// The `task fix --pr` sink (issue 313): pushes a `ready` fix task's branch onto its PR's own
/// branch, as a fast-forward from the head the task started at or not at all (never a force or a
/// rebase), then posts the in-task review as the review of the new head. Every check comes
/// before the push; a branch that moved on the remote leaves the task `ready` with its result in
/// `repo.git`, and the error names it and says to run `task fix --pr` again on the new head.
pub fn finish_fix(
    base_dir: &Path,
    id: &str,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
) -> Result<FixPushed> {
    let mut prepared = Prepared::open(base_dir, id)?;
    let task = prepared.record.clone();
    if task.kind != record::Kind::Fix {
        bail!("task {id} isn't a fix task; only `sbxm task fix` pushes onto a PR's own branch");
    }
    check_can_finish(&task)?;
    crate::commands::task_start::check_repo(&task.repo)?;
    let Some(continued) = task.continues.clone() else {
        bail!("task {id} records no PR branch to push to; see task.json");
    };
    let pr = continued.pr;
    let branch = &task.branch;
    if !repo::valid_ref_name(branch) {
        bail!(
            "task {id} records branch {branch:?}, which isn't a usable branch name; see task.json"
        );
    }
    let repo_git = prepared.meta.join("repo.git");
    if !repo_git.is_dir() {
        bail!("task {id} has no repo.git, so there is nothing to push; see `sbxm task status`");
    }
    let Some(tip) = task.tip_if_ahead(&repo_git)? else {
        bail!(
            "task {id} has no commits on {branch} beyond {}, so there is nothing to push to PR \
             #{pr}; see result.md in {}",
            continued.base,
            prepared.meta.display()
        );
    };
    refuse_workflow_changes(id, &task.changed_paths(&repo_git)?)?;
    let commits = repo::commit_lines(&repo_git, &continued.base, branch)?;
    let review = read_capped(&prepared.meta.join("review.md"))?;
    let unresolved = unresolved_reasons(&task, review.as_deref());
    let risk_text = risk_section(&task, &unresolved);
    // The review is published with the push, so a secret in it stops both; no command pushes a
    // fix task later, so the result's place is what helps.
    let secret = review
        .as_deref()
        .and_then(|text| secret_line("review.md", text))
        .or_else(|| secret_line("the risk section", &risk_text));
    if let Some(found) = secret {
        bail!(
            "{found}, so nothing was pushed to PR #{pr} and nothing posted; {}",
            kept_in(&prepared, branch)
        );
    }
    // The in-task review becomes the review of the PR's new head when it covered that commit.
    let posting = reviewed_head(&task, &tip).zip(review);
    let review_again = review::review_pr_command(pr, &task.repo, Some(&task.base), false);
    let again = format!(
        "run `sbxm task fix --pr {pr} --repo {}` again on the new head",
        task.repo
    );

    match repo::remote_branch_head(&repo_git, branch)? {
        None => bail!(
            "PR #{pr}'s branch {branch} isn't on the remote any more (deleted, or the PR \
             merged?), so nothing was pushed; {}",
            kept_in(&prepared, branch)
        ),
        // A rerun after a post that failed: the push itself is done.
        Some(head) if head == tip => {}
        Some(head) if head == continued.base => {
            repo::push_from(&repo_git, branch, &continued.base).with_context(|| {
                format!(
                    "PR #{pr}'s branch {branch} wasn't updated (sbxm never forces a push; it may \
                     have moved while the task ran); {}; {again}",
                    kept_in(&prepared, branch)
                )
            })?;
        }
        Some(head) => bail!(
            "PR #{pr}'s branch moved from {} to {head} while the task ran; {}; {again}",
            continued.base,
            kept_in(&prepared, branch)
        ),
    }

    let url = format!("https://github.com/{}/pull/{pr}", task.repo);
    prepared
        .record
        .advance(record::Stage::Finished, now(), Process::current(probe))?;
    prepared.record.pr = Some(url.clone());
    record::write(&prepared.meta, &prepared.record)
        .with_context(|| format!("{id} is pushed to PR #{pr} but task.json couldn't be updated"))?;
    let mut warnings = Vec::new();
    let review_posted = match posting {
        Some((commit, text)) => {
            let body = review::in_task_fix_comment(pr, &commit, &risk_text, &text);
            let (must_fix, level) = task
                .review
                .as_ref()
                .map_or((0, risk::Level::default()), |r| (r.must_fix, r.risk));
            let reviewed = super::pipeline::StatusInput {
                commit: Some(&commit),
                must_fix,
                text: &text,
                risk: level,
            };
            // Issue 469: the new head is reviewed now, so only `--force` reviews it again.
            let force = review::review_pr_command(pr, &task.repo, Some(&task.base), true);
            let published = review::publish(github, &task.repo, pr, &body, &reviewed, &force);
            if let Err(e) = published.comment {
                prepared.record.notes.push(format!(
                    "the in-task review was not posted on PR #{pr}: {e:#}"
                ));
                let _ = record::write(&prepared.meta, &prepared.record);
                return Err(e.context(format!(
                    "pushed {branch} to PR #{pr}, but the in-task review couldn't be posted on \
                     it; review the new head with `{review_again}`"
                )));
            }
            warnings = published.warnings;
            if let Some(review) = &mut prepared.record.review {
                review.posted = Some(commit);
            }
            record::write(&prepared.meta, &prepared.record).with_context(|| {
                format!(
                    "the in-task review is posted on PR #{pr}, but task.json couldn't be updated \
                     to say so"
                )
            })?;
            true
        }
        None => false,
    };
    Ok(FixPushed {
        url,
        branch: branch.clone(),
        commit: tip,
        commits,
        unresolved,
        review_posted,
        review_again,
        warnings,
    })
}

/// Where the `local` sink left a spec task's result.
#[derive(Debug)]
pub struct Kept {
    pub branch: String,
    pub repo_git: PathBuf,
}

/// The `local` sink for a spec task (decision 174(e), issue 143): the branch stays in the task's
/// host-owned `repo.git` and the task becomes `finished`. Nothing is pushed and nothing leaves
/// the machine; the caller says how to fetch the branch.
pub fn finish_local(base_dir: &Path, id: &str, probe: &dyn ProcessProbe) -> Result<Kept> {
    let (mut prepared, kept, _) = open_spec_result(base_dir, id, "keep")?;
    record_finished(&mut prepared, probe)?;
    Ok(kept)
}

/// The `push` sink for a spec task (decision 174(e), issue 144): the branch goes to `origin` as a
/// new branch and no PR is opened. Every check comes first: a stopped task or one with must-fix
/// findings left is refused unless `push_unresolved` (a spec task has no PR or draft to mark it
/// unfinished), and so is a branch that changes the repository's CI. A branch already on
/// `origin` at another commit is refused and never forced; one already at the task's tip (an
/// earlier push whose record was lost) is only recorded (spec section 5.4a).
pub fn finish_push(
    base_dir: &Path,
    id: &str,
    push_unresolved: bool,
    probe: &dyn ProcessProbe,
) -> Result<Kept> {
    let (mut prepared, kept, tip) = open_spec_result(base_dir, id, "push")?;
    let (branch, repo_git) = (&kept.branch, &kept.repo_git);
    if !push_unresolved {
        let unresolved = |what: String| {
            anyhow::anyhow!(
                "task {id} {what}; read review.md in its folder, or pass --push-unresolved to push \
                 it anyway"
            )
        };
        if let Some(stopped) = prepared.record.stopped {
            return Err(unresolved(format!("stopped ({})", stopped.name())));
        }
        if let Some(review) = read_capped(&prepared.meta.join("review.md"))? {
            match super::review::saved_must_fix_count(&review) {
                Some(0) => {}
                Some(n) => return Err(unresolved(format!("has {n} must-fix finding(s) left"))),
                None => {
                    return Err(unresolved(
                        "has a review.md without a must-fix count".into(),
                    ));
                }
            }
        }
    }
    refuse_workflow_changes(id, &prepared.record.changed_paths(repo_git)?)?;
    match repo::remote_branch_head(repo_git, branch)? {
        Some(head) if head == tip => {}
        Some(head) => bail!(
            "branch {branch} already exists on origin (at {head}), so task {id} would overwrite \
             someone's branch; nothing was pushed; rename or delete that branch on origin, or set \
             [finish] sink = \"local\" and fetch the result yourself"
        ),
        None => repo::push_new(repo_git, branch)?,
    }
    record_finished(&mut prepared, probe)?;
    Ok(kept)
}

/// A ready spec task with a branch worth delivering, for either sink, and the branch's tip:
/// `what` names the action in the refusals.
fn open_spec_result(base_dir: &Path, id: &str, what: &str) -> Result<(Prepared, Kept, String)> {
    let prepared = Prepared::open(base_dir, id)?;
    if prepared.record.kind != record::Kind::Spec {
        bail!("task {id} isn't a spec task; only a spec task has a [finish] sink");
    }
    check_can_finish(&prepared.record)?;
    let branch = prepared.record.branch.clone();
    if !repo::valid_ref_name(&branch) {
        bail!(
            "task {id} records branch {branch:?}, which isn't a usable branch name; see task.json"
        );
    }
    let repo_git = prepared.meta.join("repo.git");
    if !repo_git.is_dir() {
        bail!("task {id} has no repo.git, so there is no result to {what}; see `sbxm task status`");
    }
    let Some(tip) = prepared.record.tip_if_ahead(&repo_git)? else {
        bail!(
            "task {id} has no commits on {branch} beyond {}, so there is nothing to {what}",
            prepared.record.base
        );
    };
    Ok((prepared, Kept { branch, repo_git }, tip))
}

fn record_finished(prepared: &mut Prepared, probe: &dyn ProcessProbe) -> Result<()> {
    prepared
        .record
        .advance(record::Stage::Finished, now(), Process::current(probe))?;
    record::write(&prepared.meta, &prepared.record)
}

/// The note `finish` leaves on a continued task whose commits are pushed but whose PR comment
/// isn't posted yet.
const PUSHED_UNCOMMENTED_NOTE: &str = "the commits are pushed to the PR's branch but the PR isn't commented on; run `sbxm task finish` again";

/// The longest commit subject the comment on a continued PR shows in full.
const SUBJECT_CAP: usize = 500;

/// The comment on a continued PR: what the task left `unresolved` (decision 177(r), a section
/// like a draft's), the commits the task added, and `Fixes #n` for its issue. It
/// stays below what GitHub accepts: a subject over [`SUBJECT_CAP`] characters is cut, commits
/// that don't fit are counted instead of listed, and a note says so. Mentions are neutralised
/// as in the review's comment, so an agent's commit subject notifies no one.
pub fn pr_comment(
    number: u32,
    branch: &str,
    commits: &[String],
    unresolved: Option<&str>,
) -> String {
    let mut body =
        unresolved.map_or_else(String::new, |text| defang_mentions(&format!("{text}\n")));
    body.push_str(&defang_mentions(&format!(
        "`sbxm task finish` pushed {} commit(s) to {branch} for issue #{number}:\n\n",
        commits.len()
    )));
    let (mut cut, mut listed) = (0, 0);
    for line in commits {
        let (id, subject) = line.split_once(' ').unwrap_or((line, ""));
        let subject = match subject.char_indices().nth(SUBJECT_CAP) {
            Some((at, _)) => {
                cut += 1;
                format!("{}…", &subject[..at])
            }
            None => subject.to_owned(),
        };
        let item = defang_mentions(&format!("- `{id}` {subject}\n"));
        if body.len() + item.len() > COMMENT_CAP {
            break;
        }
        body.push_str(&item);
        listed += 1;
    }
    if listed < commits.len() {
        body.push_str(&format!(
            "- … and {} more commit(s), not listed here; see the PR's commits\n",
            commits.len() - listed
        ));
    }
    if cut > 0 {
        body.push_str(&format!(
            "\n(sbxm cut {cut} commit subject(s) at {SUBJECT_CAP} characters.)\n"
        ));
    }
    body.push_str(&format!("\nFixes #{number}\n"));
    body
}

/// [`finish`] for a task that continues an open PR (decision 169 (e)): pushes to the PR's branch,
/// as a fast-forward from the head the task started at and never a force, then comments on the
/// PR; no PR is opened. The remote branch must still be where the task started, checked again
/// atomically by the push itself ([`repo::push_from`]); a remote already at the task's tip means
/// a rerun after a failed comment, which only retries the comment.
fn finish_continued(
    mut prepared: Prepared,
    repo_git: &Path,
    continued: &record::PrBranch,
    tip: &str,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
) -> Result<Finished> {
    let task = prepared.record.clone();
    let (id, pr, branch) = (&task.id, continued.pr, &task.branch);
    let tip = tip.to_owned();
    let commits = repo::commit_lines(repo_git, &continued.base, branch)?;
    let review = read_capped(&prepared.meta.join("review.md"))?;
    let unresolved = unresolved_reasons(&task, review.as_deref());
    let open = if unresolved.is_empty() {
        Vec::new()
    } else {
        open_findings(&task, &prepared.meta, review.as_deref())
    };
    refuse_secret_findings(&open)?;
    let section = (!unresolved.is_empty()).then(|| {
        unresolved_text(
            "sbxm doesn't make a PR it continues a draft, but this task left:",
            &unresolved,
            &open,
        )
    });
    let body = pr_comment(task.number, branch, &commits, section.as_deref());
    refuse_secrets("the PR comment", &body)?;
    let pushed = match repo::remote_branch_head(repo_git, branch)? {
        None => bail!(
            "PR #{pr}'s branch {branch} isn't on the remote any more (deleted, or the PR merged?); \
             sbxm never forces a push, so check PR #{pr}, then push the task's commits from its \
             repo.git yourself or start the task again with `sbxm task start --restart --issue {}`",
            task.number
        ),
        Some(head) if head == tip => true,
        Some(head) if head == continued.base => false,
        Some(head) => bail!(
            "PR #{pr}'s branch {branch} moved on the remote (it is at {head}, the task started at \
             {}); sbxm never forces a push, so start the task again from the new head with \
             `sbxm task start --restart --issue {}`",
            continued.base,
            task.number
        ),
    };

    if !pushed {
        repo::push_from(repo_git, branch, &continued.base).with_context(|| {
            format!(
                "PR #{pr}'s branch {branch} wasn't updated; sbxm never forces a push, so check \
                 PR #{pr} and, if someone else pushed, start the task again from the new head \
                 with `sbxm task start --restart --issue {}`",
                task.number
            )
        })?;
    }
    if let Err(e) = github.pr_comment(&task.repo, pr, &body) {
        if !prepared
            .record
            .notes
            .iter()
            .any(|n| n == PUSHED_UNCOMMENTED_NOTE)
        {
            prepared
                .record
                .notes
                .push(PUSHED_UNCOMMENTED_NOTE.to_owned());
            let _ = record::write(&prepared.meta, &prepared.record);
        }
        return Err(e).context(format!(
            "pushed {branch} but could not comment on PR #{pr}; run `sbxm task finish --issue {}` again, which only retries the comment",
            task.number
        ));
    }
    let url = format!("https://github.com/{}/pull/{pr}", task.repo);
    prepared
        .record
        .notes
        .retain(|n| n != PUSHED_UNCOMMENTED_NOTE);
    prepared
        .record
        .advance(record::Stage::Finished, now(), Process::current(probe))?;
    prepared.record.pr = Some(url.clone());
    record::write(&prepared.meta, &prepared.record)
        .with_context(|| format!("{id} is pushed to PR #{pr} but task.json couldn't be updated"))?;
    let (summarized, warnings) = post_issue_summaries(&mut prepared, &[task.number], &url, github);
    Ok(Finished {
        // The task's base is the one it was started with, not necessarily the PR's: the review
        // diffs against the repo's default branch, as before, unless `--base` is added.
        review_again: review::review_pr_command(pr, &task.repo, None, false),
        url,
        branch: task.branch.clone(),
        cuts: Vec::new(),
        continued: Some(pr),
        adopted: false,
        draft: false,
        unresolved,
        review_posted: None,
        warnings,
        summaries_only: false,
        summarized,
        left_open: Vec::new(),
    })
}
