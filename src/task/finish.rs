//! Ending a task (spec §4, decisions 153, 158): discarding one (`task rm`, `task start
//! --restart`) and, further down, handing a ready one to GitHub (`task finish`).
//!
//! Removal only ever touches a task's own folders, whose paths are built from a validated id and
//! checked again on disk before anything is deleted: never a link, never a folder that doesn't
//! resolve to its expected place under the base dir.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use super::record::{self, ProcessProbe, Record, Status};
use crate::backend::SandboxBackend;

/// Everything `task rm` would delete, worked out before anything is.
#[derive(Debug)]
pub struct RemovalPlan {
    pub id: String,
    base_dir: PathBuf,
    /// Worker's then reviewer's, as `task.json` names them.
    pub sandboxes: Vec<String>,
    /// The folders that exist, the task folder last.
    pub folders: Vec<PathBuf>,
    /// Set when `task.json` couldn't be read, so no sandbox names are known.
    pub note: Option<String>,
}

impl RemovalPlan {
    /// The exact things that will go, one per line, for the confirmation.
    pub fn listing(&self) -> String {
        let mut lines: Vec<String> = self
            .sandboxes
            .iter()
            .map(|s| format!("  sandbox {s}"))
            .collect();
        lines.extend(self.folders.iter().map(|f| format!("  {}", f.display())));
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

/// Checks that `id` can be discarded and lists what that removes. The id is validated before any
/// path is built from it; a task whose process is alive is refused.
pub fn plan_removal(base_dir: &Path, id: &str, probe: &dyn ProcessProbe) -> Result<RemovalPlan> {
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

    let json = meta.join("task.json");
    let mut sandboxes = Vec::new();
    let mut note = None;
    if json.is_file() && schema_too_new(&json) {
        // Only a newer sbxm knows what is in it; don't delete a task this one can't read.
        record::read(&json)?;
    }
    match record::read(&json) {
        Ok(task) => {
            if task.status == Status::Running && !task.is_interrupted(probe) {
                bail!(
                    "task {id} is running (stage {}); wait for it to finish, or stop it, then run `sbxm task rm` again",
                    task.stage.name()
                );
            }
            sandboxes = sandbox_names(id, &task)?;
        }
        Err(e) => {
            note = Some(format!(
                "task.json can't be read ({e:#}), so its sandboxes can't be named; check `sbx ls` for sbxm-task-{id}-*"
            ));
        }
    }
    Ok(RemovalPlan {
        id: id.to_owned(),
        base_dir: base_dir.to_path_buf(),
        sandboxes,
        folders,
        note,
    })
}

fn sandbox_names(id: &str, task: &Record) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for agent in [&task.worker, &task.reviewer].into_iter().flatten() {
        check_sandbox_name(id, &agent.sandbox)?;
        names.push(agent.sandbox.clone());
    }
    Ok(names)
}

/// Refuses a folder that is a link, or whose real place isn't `<base>/<rel...>`.
fn check_own_folder(base_dir: &Path, dir: &Path) -> Result<()> {
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

/// Removes what `plan` lists, best effort: sandboxes first, then the clones, then the task folder,
/// which stays if anything else did (it holds the sandbox names a second `rm` needs).
pub fn remove(plan: &RemovalPlan, backend: &dyn SandboxBackend) -> RemovalReport {
    let mut report = RemovalReport::default();
    for sandbox in &plan.sandboxes {
        match crate::commands::rm::remove_sandbox(backend, sandbox) {
            Ok(()) => report.removed.push(format!("sandbox {sandbox}")),
            Err(e) => report
                .stayed
                .push(format!("sandbox {sandbox} stayed: {e:#}")),
        }
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
            fs::remove_dir_all(folder)
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
