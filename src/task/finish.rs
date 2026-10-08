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

use anyhow::{Context, Result, bail};

use super::machine;
use super::pipeline::Prepared;
use super::record::{self, Process, ProcessProbe, Record};
use super::repo;
use super::review::{COMMENT_CAP, defang_mentions};
use crate::backend::SandboxBackend;
use crate::confirm::Confirm;
use crate::github::{GitHubBackend, PrRequest};
use crate::run::results::now;

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
            let state = machine::State {
                stage: task.stage,
                status: task.status,
                interrupted: task.is_interrupted(probe),
                kind: task.kind,
            };
            if !machine::verdict(&state, machine::Event::Rm) {
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

/// A spec task's result under the `local` sink lives only in its `repo.git`, which removal deletes
/// (decision 174(e), spec section 5.4). Refused while its branch has commits beyond the base that
/// `checkout` (the user's git checkout) doesn't have, so a result is never lost by accident; the
/// caller skips this for `--force`. Only reads: `repo.git` is host-owned (decision 159) and
/// `checkout` is the user's own.
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
    if task.commits_ahead(&repo_git)? == 0 {
        return Ok(());
    }
    let tip = repo::branch_tip(&repo_git, &task.branch)?;
    let object = format!("{tip}^{{commit}}");
    if crate::git::user_run(checkout, &["cat-file", "-e", &object]).is_ok() {
        return Ok(());
    }
    let branch = &task.branch;
    bail!(
        "task {id}'s result, branch {branch}, exists only in {}, which `task rm` deletes; fetch it \
         first with: git fetch {} {branch}:{branch} (in your checkout), {force}",
        repo_git.display(),
        crate::commands::task_start::path_arg(&repo_git)
    )
}

/// Shows what removing `id` would delete, asks (unless `yes`), removes it and says what went and
/// what stayed. Nothing is touched before the answer. Fails if anything stayed.
pub fn discard(
    base_dir: &Path,
    id: &str,
    yes: bool,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    confirm: &dyn Confirm,
    out: &mut dyn Write,
) -> Result<()> {
    discard_many(
        base_dir,
        &[id.to_owned()],
        yes,
        backend,
        probe,
        confirm,
        out,
    )
}

/// [`discard`] for several tasks at once (`start --restart --issue 1 --issue 2`): every task is
/// planned first, all of them are listed and confirmed with ONE question, and only then is the
/// first deleted, so a "no" (or a task that can't be planned) deletes nothing at all.
pub fn discard_many(
    base_dir: &Path,
    ids: &[String],
    yes: bool,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    confirm: &dyn Confirm,
    out: &mut dyn Write,
) -> Result<()> {
    let plans = ids
        .iter()
        .map(|id| plan_removal(base_dir, id, probe))
        .collect::<Result<Vec<_>>>()?;
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
            bail!("refusing to remove {what} without a terminal; pass --yes to delete:\n{listing}");
        }
        if !confirm.confirm(&format!(
            "Permanently delete {what}: these sandboxes and folders?\n{listing}\n"
        ))? {
            bail!("removing {what} cancelled; nothing was deleted");
        }
    }
    let mut stayed = 0;
    for plan in &plans {
        let report = remove(plan, backend);
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

/// The longest piece of `result.md` or `review.md` put in a PR body, in characters (GitHub allows
/// 65,536 for the whole body).
pub const SECTION_CAP: usize = 25_000;

/// The PR body: `Fixes #N`, then each file under a heading. A file longer than the cap is cut and
/// the body says so. Returns the body and one line per cut for the command to print.
pub fn pr_body(number: u32, result: Option<&str>, review: Option<&str>) -> (String, Vec<String>) {
    let mut body = format!("Fixes #{number}\n");
    let mut cuts = Vec::new();
    for (file, heading, text) in [
        ("result.md", "Result", result),
        ("review.md", "Review", review),
    ] {
        body.push_str(&format!("\n## {heading}\n\n"));
        let Some(text) = text else {
            body.push_str(&format!("(no {file})\n"));
            continue;
        };
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
        record::Kind::Issue | record::Kind::Spec => {}
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

fn refuse_secrets(name: &str, text: &str) -> Result<()> {
    for (n, line) in text.lines().enumerate() {
        if let Some(kind) = super::findings::secret_kind(line) {
            bail!(
                "{name} line {} looks like a secret ({kind}); remove it from the file in the task folder, then run `sbxm task finish` again",
                n + 1
            );
        }
    }
    Ok(())
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
    let mut prepared = Prepared::open(base_dir, id)?;
    if prepared.record.kind == record::Kind::Spec {
        bail!("task {id} is a spec task; it finishes through its [finish] sink, never a PR");
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
    if task.commits_ahead(&repo_git)? == 0 {
        let from = task.continues.as_ref().map_or(&task.base, |c| &c.base);
        bail!(
            "task {id} has no commits on {} beyond {from}, so there is nothing to publish",
            task.branch,
        );
    }
    refuse_workflow_changes(id, &task.changed_paths(&repo_git)?)?;
    if let Some(continued) = &task.continues {
        return finish_continued(prepared, &repo_git, continued, github, probe);
    }
    let result = read_capped(&prepared.meta.join("result.md"))?;
    let review = read_capped(&prepared.meta.join("review.md"))?;
    for (name, text) in [("result.md", &result), ("review.md", &review)] {
        if let Some(text) = text {
            refuse_secrets(name, text)?;
        }
    }
    let (body, cuts) = pr_body(task.number, result.as_deref(), review.as_deref());

    repo::push(&repo_git, &task.branch)?;
    let request = PrRequest {
        head: task.branch.clone(),
        base: task.base.clone(),
        title: task.title.clone(),
        body,
    };
    let url = match github.pr_create(&task.repo, &request) {
        Ok(url) => url,
        Err(e) => {
            if !prepared.record.notes.iter().any(|n| n == PUSHED_NOTE) {
                prepared.record.notes.push(PUSHED_NOTE.to_owned());
                let _ = record::write(&prepared.meta, &prepared.record);
            }
            return Err(e).context(format!(
                "pushed {} but could not open the PR; run `sbxm task finish --issue {}` again, which only retries the PR",
                task.branch, task.number
            ));
        }
    };
    prepared.record.notes.retain(|n| n != PUSHED_NOTE);
    prepared
        .record
        .advance(record::Stage::Finished, now(), Process::current(probe))?;
    prepared.record.pr = Some(url.clone());
    record::write(&prepared.meta, &prepared.record).with_context(|| {
        format!("the PR is open at {url} but task.json couldn't be updated; note the URL")
    })?;
    Ok(Finished {
        url,
        branch: task.branch,
        cuts,
        continued: None,
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
    let (mut prepared, kept) = open_spec_result(base_dir, id, "keep")?;
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
    let (mut prepared, kept) = open_spec_result(base_dir, id, "push")?;
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
    let tip = repo::branch_tip(repo_git, branch)?;
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

/// A ready spec task with a branch worth delivering, for either sink: `what` names the action in
/// the refusals.
fn open_spec_result(base_dir: &Path, id: &str, what: &str) -> Result<(Prepared, Kept)> {
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
    if prepared.record.commits_ahead(&repo_git)? == 0 {
        bail!(
            "task {id} has no commits on {branch} beyond {}, so there is nothing to {what}",
            prepared.record.base
        );
    }
    Ok((prepared, Kept { branch, repo_git }))
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

/// The comment on a continued PR: the commits the task added, and `Fixes #n` for its issue. It
/// stays below what GitHub accepts: a subject over [`SUBJECT_CAP`] characters is cut, commits
/// that don't fit are counted instead of listed, and a note says so. Mentions are neutralised
/// as in the review's comment, so an agent's commit subject notifies no one.
pub fn pr_comment(number: u32, branch: &str, commits: &[String]) -> String {
    let mut body = defang_mentions(&format!(
        "`sbxm task finish` pushed {} commit(s) to {branch} for issue #{number}:\n\n",
        commits.len()
    ));
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
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
) -> Result<Finished> {
    let task = prepared.record.clone();
    let (id, pr, branch) = (&task.id, continued.pr, &task.branch);
    let tip = repo::branch_tip(repo_git, branch)?;
    let commits = repo::commit_lines(repo_git, &continued.base, branch)?;
    let body = pr_comment(task.number, branch, &commits);
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
    Ok(Finished {
        url,
        branch: task.branch.clone(),
        cuts: Vec::new(),
        continued: Some(pr),
    })
}
