//! `sbxm task` orchestration, one function per phase (spec §5, §1). Every refusal happens before
//! the first write or backend call; once folders are reserved, a failure removes them again.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use std::time::Instant;

use super::config::TaskConfig;
use super::gates::{self, GateOutcome, HostRunner};
use super::machine;
use super::prompts::{self, Role};
use super::record::{
    self, Agent, GateResult, GateRun, Kind, NewTask, OpenFinding, PrBranch, Process, ProcessProbe,
    Record, RunInfo, Stage, Status,
};
use super::repo::{self, AgentFile, BUNDLE_CAP, Existing, Identity};
use super::review;
use super::risk;
use super::select::{self, Selection};
use crate::backend::{CreateSpec, ExecSpec, SandboxBackend, Stdin};
use crate::config::{self, GlobalConfig, Profile, Resources};
use crate::github::{GitHubBackend, Issue, IssueText, PrInfo, PrState};
use crate::harness::Harness;
use crate::headless::{self, HeadlessOpts, RunStatus};
use crate::run::kits::{self, HarnessKits, Overrides};
use crate::run::orchestrate::in_sandbox_path;
use crate::run::results::now;

/// Folder inside the workspace for sbxm's files (the issue, the prompt, `result.md`, the bundle).
pub const AGENT_DIR: &str = ".sbxm-task";

/// What every phase of a task needs; the command layer builds it from flags and the environment.
pub struct Ctx<'a> {
    pub config_dir: &'a Path,
    /// `owner/name`.
    pub repo: &'a str,
    /// What `repo.git` is cloned from: the GitHub URL (a local path in tests).
    pub clone_source: &'a str,
    pub base_branch: &'a str,
    pub config: &'a TaskConfig,
    pub backend: &'a dyn SandboxBackend,
    pub github: &'a dyn GitHubBackend,
    /// The committer identity the agent's commits carry.
    pub identity: &'a Identity,
    pub probe: &'a dyn ProcessProbe,
    /// Runs host-tier gates (never the agent's own commands anywhere else).
    pub host: &'a dyn HostRunner,
}

/// A task that is ready for its worker.
#[derive(Debug)]
pub struct Prepared {
    pub record: Record,
    /// `<base>/.sbxm/tasks/<id>/`
    pub meta: PathBuf,
    /// `<base>/tasks/<id>/`: the worker's clone, mounted in its sandbox.
    pub workspace: PathBuf,
    /// The worker's kits; `None` for a task reopened from its record.
    pub kits: Option<HarnessKits>,
    /// One line each, to print before the run.
    pub warnings: Vec<String>,
}

impl Prepared {
    /// A task that already exists, read back from its record (`<base>/.sbxm/tasks/<id>/`).
    /// `id` must look like `issue-<n>` or `pr-<n>` before any path is built from it.
    pub fn open(base_dir: &Path, id: &str) -> Result<Self> {
        if !record::is_valid_id(id) {
            bail!("{id:?} isn't a task id; task ids look like issue-41 or pr-7");
        }
        let meta = record::task_dir(base_dir, id);
        let path = meta.join("task.json");
        if !path.is_file() {
            bail!("no task {id}; run `sbxm task status` to list the tasks");
        }
        Ok(Self {
            record: record::read(&path)?,
            meta,
            workspace: base_dir.join("tasks").join(id),
            kits: None,
            warnings: Vec::new(),
        })
    }
}

/// Whether gates may run on this task now (`task gates`): after the worker or the fix round has
/// finished, or again after earlier gates ended. Decided through `machine::TABLE`; the reason for
/// a refusal says what to do.
pub fn check_can_gate(task: &Record, probe: &dyn ProcessProbe) -> Result<()> {
    let id = &task.id;
    let number = task.number;
    let state = machine::State {
        stage: task.stage,
        status: task.status,
        interrupted: task.is_interrupted(probe),
        kind: task.kind,
    };
    if machine::verdict(&state, machine::Event::Gates) {
        return Ok(());
    }
    match (task.stage, task.status) {
        (Stage::Working | Stage::Fixing, Status::Running) => {
            if task.is_interrupted(probe) {
                bail!(
                    "task {id} was interrupted while its {} stage was running; see `sbxm task status --issue {number}`",
                    task.stage.name()
                )
            }
            bail!(
                "the worker is still running for task {id} (worker still running); wait for `sbxm task start` to finish"
            )
        }
        (Stage::Working | Stage::Fixing, _) => {
            bail!(
                "the worker failed for task {id}, so there is nothing to gate; see `sbxm task status --issue {number}`"
            )
        }
        (Stage::Gating, _) => bail!(
            "gates are running for task {id}, or were interrupted; see `sbxm task status --issue {number}`"
        ),
        (stage, _) => bail!(
            "task {id} is at stage {}; gates run after the worker or after the fix round",
            stage.name()
        ),
    }
}

/// What running gates and reviews need from the outside world (less than a whole [`Ctx`]: no
/// GitHub, no clone source).
pub struct TaskEnv<'a> {
    pub config_dir: &'a Path,
    pub config: &'a TaskConfig,
    pub backend: &'a dyn SandboxBackend,
    pub probe: &'a dyn ProcessProbe,
    pub host: &'a dyn HostRunner,
}

impl Ctx<'_> {
    pub fn env(&self) -> TaskEnv<'_> {
        TaskEnv {
            config_dir: self.config_dir,
            config: self.config,
            backend: self.backend,
            probe: self.probe,
            host: self.host,
        }
    }
}

/// Spec §5.2 "PR": a task for reviewing an open pull request from a branch of this same repo.
/// Everything that can be refused is refused first (a fork, a closed or merged PR, an existing
/// task, a missing reviewer secret, a bad profile); then the task folder, `repo.git` with the
/// PR's head as the local branch `pr-<n>`, the review context (the PR and the issues it closes)
/// and the first record. No worker, no clone, no sandbox yet; a failure removes the folder again.
pub fn prepare_pr(ctx: &Ctx, pr: &PrInfo) -> Result<Prepared> {
    let global = GlobalConfig::load(ctx.config_dir)?;
    let number = pr.number;
    let id = format!("pr-{number}");
    let meta = record::task_dir(&global.base_dir, &id);
    let workspace = global.base_dir.join("tasks").join(&id);

    match pr.state {
        PrState::Open => {}
        PrState::Closed => bail!("PR #{number} is closed; only open PRs are reviewed"),
        PrState::Merged => bail!("PR #{number} is merged; only open PRs are reviewed"),
    }
    if pr.is_cross_repository {
        bail!(
            "PR #{number} comes from a fork; only PRs from branches of {} are reviewed, because a \
             fork's code is not something to fetch and run here (decision 87)",
            ctx.repo
        );
    }
    if meta.exists() {
        let stage = record::read(&meta.join("task.json"))
            .map(|r| r.stage.name().to_owned())
            .unwrap_or_else(|_| "no readable record".to_owned());
        bail!(
            "task {id} exists (stage {stage}); use `sbxm task status`, or `sbxm task rm --pr {number}` to remove it first"
        );
    }
    let reviewer = &ctx.config.reviewer;
    let profile = Profile::load(global.profiles_dir(), &ctx.config.sandbox.profile)?;
    check_secrets(
        ctx.backend,
        reviewer.harness,
        "the reviewer",
        &ctx.config.sandbox.profile,
        &profile,
    )?;
    let mut warnings = ctx.config.warnings.clone();
    warnings.extend(
        reviewer
            .harness
            .unsupported(&profile, "the reviewer's sandbox"),
    );

    let parent = meta.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("cannot create {}", parent.display()))?;
    fs::create_dir(&meta).with_context(|| format!("cannot reserve {}", meta.display()))?;
    let built = (|| -> Result<Record> {
        // The reviewer's kits must validate before anything else is made.
        let kit_set = kits::build_for(
            ctx.config_dir,
            &ctx.config.sandbox.profile,
            &[reviewer.harness],
            &Overrides {
                cpus: ctx.config.sandbox.cpus,
                memory: ctx.config.sandbox.memory.clone(),
            },
            &meta.join("kits-review"),
            ctx.backend,
        )?;
        let config_hash = kit_set
            .get(reviewer.harness)
            .map(|k| k.config_hash.clone())
            .context("no kits were built for the reviewer's harness")?;

        let repo_git = meta.join("repo.git");
        repo::clone_bare(ctx.clone_source, &repo_git)?;
        let branch = repo::fetch_pr_head(&repo_git, number)?;

        // The review context: the PR itself, then each issue it closes.
        let mut context = format!(
            "Pull request #{number}: {}\nBranch: {}\n\n{}\n",
            pr.title, pr.head_ref, pr.body
        );
        for issue_number in &pr.closing_issues {
            let issue = ctx.github.issue(ctx.repo, *issue_number).with_context(|| {
                format!("cannot read issue #{issue_number}, which the pull request closes")
            })?;
            context.push_str(&format!(
                "\n---\n\nIssue #{issue_number} (closed by this pull request):\n\n{}\n",
                issue.text
            ));
        }
        fs::write(meta.join("issue.md"), &context)?;

        let mut task = Record::new(
            &NewTask {
                kind: Kind::Pr,
                number,
                repo: ctx.repo,
                title: &pr.title,
                base: ctx.base_branch,
                branch: &branch,
                config_hash: &config_hash,
                id: None,
            },
            now(),
            Process::current(ctx.probe),
        );
        // A PR task has no worker, so it never fixes anything (decision 173(d)).
        task.fix_rounds = 0;
        // An issue task for something this PR closes is related work (spec §5.2).
        task.related = pr
            .closing_issues
            .iter()
            .copied()
            .filter(|n| {
                record::task_dir(&global.base_dir, &format!("issue-{n}"))
                    .join("task.json")
                    .is_file()
            })
            .collect();
        record::write(&meta, &task)?;
        Ok(task)
    })();

    match built {
        Ok(record) => Ok(Prepared {
            record,
            meta,
            workspace,
            kits: None,
            warnings,
        }),
        Err(e) => {
            let _ = fs::remove_dir_all(&meta);
            Err(e)
        }
    }
}

/// `--restart` deletes a task and then starts the issue again, so before the first deletion every
/// issue in `restarting` (those that have a task) must be able to start: open, not a question, not
/// blocked, not related to another task. Otherwise the restart would leave the user with nothing.
/// The tasks being restarted don't count as "already has a task" here.
pub fn check_restartable(ctx: &Ctx, restarting: &[u32]) -> Result<()> {
    if restarting.is_empty() {
        return Ok(());
    }
    let base = GlobalConfig::load(ctx.config_dir)?.base_dir;
    let open = ctx.github.issues_open(ctx.repo)?;
    let taken: Vec<u32> = record::load_all(&base)?
        .into_iter()
        .filter(|r| r.kind == Kind::Issue && !restarting.contains(&r.number))
        .map(|r| r.number)
        .collect();
    let selection = select::select(
        &open,
        &taken,
        Some(restarting),
        restarting.len(),
        &HashMap::new(),
    );
    let mut problems: Vec<String> = selection
        .not_open
        .iter()
        .map(|n| format!("#{n} isn't an open issue"))
        .collect();
    problems.extend(
        selection
            .skips
            .iter()
            .map(|(n, why)| format!("#{n}: {why}")),
    );
    if !problems.is_empty() {
        bail!(
            "--restart would delete a task it then could not start again ({}); fix that, or remove \
             the task with `sbxm task rm --issue <n>`",
            problems.join("; ")
        );
    }
    Ok(())
}

/// The state of every PR an open `must-fix` issue names on its first line (`PR: #n`), so selection
/// can put those of an open PR first (decision 169). A PR that can't be read (deleted, a typo, a
/// network error) is not an error here: it is left out of the map, `select` skips the issues that
/// name it, and the warning says why, so one stale reference never stops the other issues and
/// nothing starts whose PR isn't known to be open.
fn must_fix_pr_states(
    ctx: &Ctx,
    open: &[Issue],
    taken: &[u32],
) -> (HashMap<u32, PrState>, Vec<String>) {
    let mut named = select::needs_pr_state(open, taken);
    named.sort_unstable();
    let mut states = HashMap::new();
    let mut warnings = Vec::new();
    let mut unreadable: Vec<u32> = Vec::new();
    for (pr, issue) in named {
        if unreadable.contains(&pr) {
            warnings.push(format!(
                "#{issue}: PR #{pr} couldn't be read (see above); skipped"
            ));
        } else if let std::collections::hash_map::Entry::Vacant(slot) = states.entry(pr) {
            match ctx.github.pr(ctx.repo, pr) {
                Ok(info) => {
                    slot.insert(info.state);
                }
                Err(e) => {
                    unreadable.push(pr);
                    warnings.push(format!("#{issue}: couldn't read PR #{pr} ({e:#}); skipped"));
                }
            }
        }
    }
    (states, warnings)
}
/// Chooses the issues to start (spec §8) and refuses when there is nothing to pick.
pub fn select_issues(ctx: &Ctx, explicit: Option<&[u32]>, workers: usize) -> Result<Selection> {
    require_picks(choose_issues(ctx, explicit, workers)?)
}

/// Applies the selection rules and returns the result whether or not anything was picked, so a
/// caller can show the warnings and skips before it refuses (see `require_picks`).
pub fn choose_issues(ctx: &Ctx, explicit: Option<&[u32]>, workers: usize) -> Result<Selection> {
    let base = GlobalConfig::load(ctx.config_dir)?.base_dir;
    let open = ctx.github.issues_open(ctx.repo)?;
    let taken: Vec<u32> = record::load_all(&base)?
        .into_iter()
        .filter(|r| r.kind == Kind::Issue)
        .map(|r| r.number)
        .collect();
    let (prs, warnings) = if explicit.is_none() {
        must_fix_pr_states(ctx, &open, &taken)
    } else {
        (HashMap::new(), Vec::new())
    };
    let mut selection = select::select(&open, &taken, explicit, workers, &prs);
    selection.warnings = warnings;
    Ok(selection)
}

/// With nothing to pick, the error says why for each candidate.
pub fn require_picks(selection: Selection) -> Result<Selection> {
    if selection.picks.is_empty() {
        let mut lines: Vec<String> = selection
            .not_open
            .iter()
            .map(|n| format!("#{n} isn't an open issue"))
            .collect();
        lines.extend(
            selection
                .skips
                .iter()
                .map(|(n, why)| format!("#{n}: {why}")),
        );
        if lines.is_empty() {
            lines.push("no open issues".to_owned());
        }
        bail!("nothing to start: {}", lines.join("; "));
    }
    Ok(selection)
}

/// The harness's provider secret and the profile's own `secrets.services` must all be stored in
/// `sbx` before anything is created; the error says which and how to add it.
fn check_secrets(
    backend: &dyn SandboxBackend,
    harness: Harness,
    who: &str,
    profile_name: &str,
    profile: &Profile,
) -> Result<()> {
    let stored = backend.secret_services()?;
    let mut needed = vec![(
        harness.provider_secret().to_owned(),
        format!("{who}, {}", harness.as_str()),
    )];
    needed.extend(profile.secrets.services.iter().map(|s| {
        (
            s.clone(),
            format!("secrets.services of profile '{profile_name}'"),
        )
    }));
    if let Some((missing, reason)) = needed.iter().find(|(s, _)| !stored.contains(s)) {
        bail!(
            "secret '{missing}' (needed by {reason}) is not stored in sbx; add it with \
             `sbx secret set {missing}` or import it with `sbx setup`"
        );
    }
    Ok(())
}

fn bullets(commands: &[String]) -> String {
    if commands.is_empty() {
        "(none configured)".to_owned()
    } else {
        commands
            .iter()
            .map(|c| format!("- `{c}`"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// The PR an issue belongs to: `PR: #n` on the first line of its body (decision 169), the body
/// being what follows the `--` line of `gh issue view`'s output.
pub fn issue_pr(issue: &IssueText) -> Option<u32> {
    let body = issue
        .text
        .split_once("\n--\n")
        .map_or(issue.text.as_str(), |(_, body)| body);
    super::findings::pr_of(body)
}

/// The branch of the open PR that issue `number` belongs to (decision 169 (d)), read before any
/// write. A closed or merged PR, or one from a fork, is refused: there is no branch to continue.
fn pr_to_continue(ctx: &Ctx, number: u32, pr: u32) -> Result<String> {
    let info = ctx.github.pr(ctx.repo, pr).with_context(|| {
        format!(
            "issue #{number} belongs to PR #{pr} (`PR: #{pr}` in its body), which cannot be read"
        )
    })?;
    let fix = format!(
        "remove the `PR: #{pr}` line from issue #{number} to start it from the base branch instead"
    );
    match info.state {
        PrState::Open => {}
        PrState::Closed => {
            bail!(
                "issue #{number} belongs to PR #{pr}, which is closed, so it has no branch to continue; {fix}"
            )
        }
        PrState::Merged => {
            bail!(
                "issue #{number} belongs to PR #{pr}, which is merged, so it has no branch to continue; {fix}"
            )
        }
    }
    if info.is_cross_repository {
        bail!(
            "issue #{number} belongs to PR #{pr}, which comes from a fork; only branches of {} are \
             continued, because a fork's code is not something to fetch and run here (decision 87); {fix}",
            ctx.repo
        );
    }
    if !repo::valid_ref_name(&info.head_ref) {
        bail!(
            "issue #{number} belongs to PR #{pr}, whose branch {:?} isn't a usable branch name; {fix}",
            info.head_ref
        );
    }
    Ok(info.head_ref)
}

/// The PR whose branch an issue's task continues, as (PR number, its branch), or `None` when the
/// issue names no PR; an error when it names one that can't be continued. `prepare` and
/// `task start --restart` both use it, so a restart never deletes a task it couldn't start again.
pub fn continued_pr(ctx: &Ctx, issue: &IssueText) -> Result<Option<(u32, String)>> {
    match issue_pr(issue) {
        Some(pr) => Ok(Some((pr, pr_to_continue(ctx, issue.number, pr)?))),
        None => Ok(None),
    }
}

/// Spec §5.1 steps 1 and 2: checks, then the task's folders, `repo.git`, the worker's clone,
/// `issue.md` and the prompt, the sandbox and the first record.
pub fn prepare(ctx: &Ctx, issue: &IssueText) -> Result<Prepared> {
    let global = GlobalConfig::load(ctx.config_dir)?;
    let id = format!("issue-{}", issue.number);
    let meta = record::task_dir(&global.base_dir, &id);
    let workspace = global.base_dir.join("tasks").join(&id);

    // Check before acting: nothing is written until every one of these passes.
    if issue.state != "OPEN" {
        bail!(
            "issue #{} is {}, not open; pick an open issue",
            issue.number,
            issue.state.to_lowercase()
        );
    }
    if meta.exists() || workspace.exists() {
        let stage = record::read(&meta.join("task.json"))
            .map(|r| r.stage.name().to_owned())
            .unwrap_or_else(|_| "no readable record".to_owned());
        bail!(
            "task {id} exists (stage {stage}); use `sbxm task status`, or --restart to start it again"
        );
    }
    let continues = continued_pr(ctx, issue)?;
    let worker = &ctx.config.worker;
    let profile = Profile::load(global.profiles_dir(), &ctx.config.sandbox.profile)?;
    check_secrets(
        ctx.backend,
        worker.harness,
        "the worker",
        &ctx.config.sandbox.profile,
        &profile,
    )?;
    let mut warnings = ctx.config.warnings.clone();
    warnings.extend(worker.harness.unsupported(&profile, "the worker's sandbox"));
    let template = prompts::template(Role::Worker, &ctx.config.prompts)?;
    let branch = continues
        .as_ref()
        .map_or_else(|| id.clone(), |(_, branch)| branch.clone());
    let pr_context = continues.as_ref().map_or_else(String::new, |(pr, branch)| {
        format!(
            " This issue belongs to pull request #{pr}, and {branch} is that pull request's \
             branch: it already has commits, so build on them instead of starting over."
        )
    });
    let prompt = prompts::render(
        &template.name,
        &template.text,
        &[
            ("issue", &issue.text),
            ("number", &issue.number.to_string()),
            ("branch", &branch),
            ("base", ctx.base_branch),
            ("repo", ctx.repo),
            ("pr_context", &pr_context),
            ("gates_sandbox", &bullets(&ctx.config.gates.sandbox)),
            ("gates_host", &bullets(&ctx.config.gates.host)),
        ],
    )?;

    // Reserve both folders; `create_dir` fails if either appeared meanwhile.
    for dir in [&meta, &workspace] {
        let parent = dir.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    fs::create_dir(&meta).with_context(|| format!("cannot reserve {}", meta.display()))?;
    if let Err(e) = fs::create_dir(&workspace) {
        let _ = fs::remove_dir_all(&meta);
        return Err(e).with_context(|| format!("cannot reserve {}", workspace.display()));
    }

    let sandbox = format!("sbxm-task-{id}-{}", worker.harness.as_str());
    let built = (|| -> Result<(Record, HarnessKits)> {
        let kit_set = kits::build_for(
            ctx.config_dir,
            &ctx.config.sandbox.profile,
            &[worker.harness],
            &Overrides {
                cpus: ctx.config.sandbox.cpus,
                memory: ctx.config.sandbox.memory.clone(),
            },
            &meta.join("kits"),
            ctx.backend,
        )?;
        let harness_kits = kit_set
            .get(worker.harness)
            .cloned()
            .context("no kits were built for the worker's harness")?;

        let repo_git = meta.join("repo.git");
        repo::clone_bare(ctx.clone_source, &repo_git)?;
        let continued = match &continues {
            Some((pr, pr_branch)) => Some(PrBranch {
                pr: *pr,
                branch: pr_branch.clone(),
                base: repo::fetch_pr_branch(&repo_git, *pr, pr_branch)?,
            }),
            None => {
                repo::create_branch(&repo_git, &branch, ctx.base_branch)?;
                None
            }
        };
        repo::clone_workspace(&repo_git, &workspace, &branch, ctx.identity)?;

        // The agent's `git add -A` must not pick up sbxm's own files.
        let info = workspace.join(".git").join("info");
        fs::create_dir_all(&info)?;
        fs::write(info.join("exclude"), format!("# sbxm\n{AGENT_DIR}/\n"))?;
        repo::write_agent_files(
            &workspace,
            &[
                ("issue.md", issue.text.as_bytes()),
                ("prompt.md", prompt.as_bytes()),
            ],
            Existing::Replace,
        )?;
        fs::write(meta.join("issue.md"), &issue.text)?;
        fs::write(meta.join("worker-prompt.md"), &prompt)?;

        // The record goes first: sandbox setup takes minutes, and `status` (and a restart after a
        // kill) must see the task during it.
        let mut task = Record::new(
            &NewTask {
                kind: Kind::Issue,
                number: issue.number,
                repo: ctx.repo,
                title: &issue.title,
                base: ctx.base_branch,
                branch: &branch,
                config_hash: &harness_kits.config_hash,
                id: None,
            },
            now(),
            Process::current(ctx.probe),
        );
        task.continues = continued;
        task.fix_rounds = ctx.config.fix_rounds;
        task.worker = Some(Agent {
            harness: worker.harness.as_str().to_owned(),
            model: worker.model.clone(),
            sandbox: sandbox.clone(),
            workspace: slashes(&workspace),
            profile: Some(ctx.config.sandbox.profile.clone()),
            cpus: Some(kit_set.resources.cpus),
            memory: Some(kit_set.resources.memory.clone()),
            run: None,
        });
        record::write(&meta, &task)?;

        ctx.backend
            .create(&CreateSpec {
                name: sandbox.clone(),
                agent: worker.harness.agent_arg().into(),
                workspace: workspace.clone(),
                cpus: kit_set.resources.cpus,
                memory: kit_set.resources.memory.clone(),
                skills: harness_kits.skills_store,
                kits: harness_kits.dirs.clone(),
            })
            .with_context(|| format!("cannot create sandbox {sandbox}"))?;
        Ok((task, harness_kits))
    })();

    match built {
        Ok((record, kits)) => Ok(Prepared {
            record,
            meta,
            workspace,
            kits: Some(kits),
            warnings,
        }),
        Err(e) => {
            // Back to nothing: the sandbox (if it got created) and both folders.
            let _ = ctx.backend.remove(&sandbox);
            let _ = fs::remove_dir_all(&workspace);
            let _ = fs::remove_dir_all(&meta);
            Err(e)
        }
    }
}

/// Spec §5.1 steps 1 and 2, for a spec source (decision 174(d), issue 142): same as [`prepare`],
/// but the source is a file read from disk instead of a GitHub issue. The id and the record's
/// title come from [`record::spec_id`]; the file's text is copied in as `source.md` (in the
/// task's own folder and the worker's workspace) and the worker prompt points at that instead of
/// `issue.md`. There is no PR to continue, so `continues` is always `None`; no GitHub call is
/// made anywhere in this function.
pub fn prepare_spec(ctx: &Ctx, source_path: &Path) -> Result<Prepared> {
    let global = GlobalConfig::load(ctx.config_dir)?;
    // A plain file copy only: a link could point anywhere the user can read, and a device or a
    // pipe could block the read or never end.
    let kind = fs::symlink_metadata(source_path)
        .with_context(|| format!("cannot read {}", source_path.display()))?
        .file_type();
    if kind.is_symlink() {
        bail!(
            "{} is a link; give sbxm the file itself, not a link to it",
            source_path.display()
        );
    }
    if !kind.is_file() {
        bail!("{} is not a regular file", source_path.display());
    }
    let (id, title) = record::spec_id(source_path)?;
    let meta = record::task_dir(&global.base_dir, &id);
    let workspace = global.base_dir.join("tasks").join(&id);

    // Check before acting: nothing is written until every one of these passes.
    let text = fs::read_to_string(source_path)
        .with_context(|| format!("cannot read {}", source_path.display()))?;
    if meta.exists() || workspace.exists() {
        let stage = record::read(&meta.join("task.json"))
            .map(|r| r.stage.name().to_owned())
            .unwrap_or_else(|_| "no readable record".to_owned());
        bail!(
            "task {id} exists (stage {stage}); use `sbxm task status`, or remove it with \
             `sbxm task rm {}` to start it again",
            crate::commands::task_start::spec_flag(source_path)
        );
    }
    let worker = &ctx.config.worker;
    let profile = Profile::load(global.profiles_dir(), &ctx.config.sandbox.profile)?;
    check_secrets(
        ctx.backend,
        worker.harness,
        "the worker",
        &ctx.config.sandbox.profile,
        &profile,
    )?;
    let mut warnings = ctx.config.warnings.clone();
    warnings.extend(worker.harness.unsupported(&profile, "the worker's sandbox"));
    let template = prompts::template(Role::WorkerSpec, &ctx.config.prompts)?;
    let prompt = prompts::render(
        &template.name,
        &template.text,
        &[
            ("branch", &id),
            ("base", ctx.base_branch),
            ("repo", ctx.repo),
            ("gates_sandbox", &bullets(&ctx.config.gates.sandbox)),
            ("gates_host", &bullets(&ctx.config.gates.host)),
        ],
    )?;

    // Reserve both folders; `create_dir` fails if either appeared meanwhile.
    for dir in [&meta, &workspace] {
        let parent = dir.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    fs::create_dir(&meta).with_context(|| format!("cannot reserve {}", meta.display()))?;
    if let Err(e) = fs::create_dir(&workspace) {
        let _ = fs::remove_dir_all(&meta);
        return Err(e).with_context(|| format!("cannot reserve {}", workspace.display()));
    }

    let sandbox = format!("sbxm-task-{id}-{}", worker.harness.as_str());
    let built = (|| -> Result<(Record, HarnessKits)> {
        let kit_set = kits::build_for(
            ctx.config_dir,
            &ctx.config.sandbox.profile,
            &[worker.harness],
            &Overrides {
                cpus: ctx.config.sandbox.cpus,
                memory: ctx.config.sandbox.memory.clone(),
            },
            &meta.join("kits"),
            ctx.backend,
        )?;
        let harness_kits = kit_set
            .get(worker.harness)
            .cloned()
            .context("no kits were built for the worker's harness")?;

        let repo_git = meta.join("repo.git");
        repo::clone_bare(ctx.clone_source, &repo_git)?;
        repo::create_branch(&repo_git, &id, ctx.base_branch)?;
        repo::clone_workspace(&repo_git, &workspace, &id, ctx.identity)?;

        // The agent's `git add -A` must not pick up sbxm's own files.
        let info = workspace.join(".git").join("info");
        fs::create_dir_all(&info)?;
        fs::write(info.join("exclude"), format!("# sbxm\n{AGENT_DIR}/\n"))?;
        repo::write_agent_files(
            &workspace,
            &[
                ("source.md", text.as_bytes()),
                ("prompt.md", prompt.as_bytes()),
            ],
            Existing::Replace,
        )?;
        fs::write(meta.join("source.md"), &text)?;
        fs::write(meta.join("worker-prompt.md"), &prompt)?;

        // The record goes first: sandbox setup takes minutes, and `status` (and a restart after a
        // kill) must see the task during it.
        let mut task = Record::new(
            &NewTask {
                kind: Kind::Spec,
                number: 0,
                repo: ctx.repo,
                title: &title,
                base: ctx.base_branch,
                branch: &id,
                config_hash: &harness_kits.config_hash,
                id: Some(&id),
            },
            now(),
            Process::current(ctx.probe),
        );
        task.fix_rounds = ctx.config.fix_rounds;
        task.worker = Some(Agent {
            harness: worker.harness.as_str().to_owned(),
            model: worker.model.clone(),
            sandbox: sandbox.clone(),
            workspace: slashes(&workspace),
            profile: Some(ctx.config.sandbox.profile.clone()),
            cpus: Some(kit_set.resources.cpus),
            memory: Some(kit_set.resources.memory.clone()),
            run: None,
        });
        record::write(&meta, &task)?;

        ctx.backend
            .create(&CreateSpec {
                name: sandbox.clone(),
                agent: worker.harness.agent_arg().into(),
                workspace: workspace.clone(),
                cpus: kit_set.resources.cpus,
                memory: kit_set.resources.memory.clone(),
                skills: harness_kits.skills_store,
                kits: harness_kits.dirs.clone(),
            })
            .with_context(|| format!("cannot create sandbox {sandbox}"))?;
        Ok((task, harness_kits))
    })();

    match built {
        Ok((record, kits)) => Ok(Prepared {
            record,
            meta,
            workspace,
            kits: Some(kits),
            warnings,
        }),
        Err(e) => {
            // Back to nothing: the sandbox (if it got created) and both folders.
            let _ = ctx.backend.remove(&sandbox);
            let _ = fs::remove_dir_all(&workspace);
            let _ = fs::remove_dir_all(&meta);
            Err(e)
        }
    }
}

/// What happened to one pick of `start`.
#[derive(Debug)]
pub struct TaskReport {
    pub number: u32,
    /// Lines to print before the run (unsupported settings, same-harness reviewer).
    pub warnings: Vec<String>,
    /// `Err` is a refusal or a failure before or while running; the task's own folders are
    /// already cleaned up when it happened before the record existed.
    pub result: Result<Worked>,
    /// The gates after the worker; `None` when the worker failed or never ran.
    pub gates: Option<Result<Gated>>,
}

/// Starts a task for each of `numbers` at the same time (`--workers`, decision 144): each gets
/// its own folders and sandbox on its own thread, and one failing does not stop the others.
/// Reports come back in the order of `numbers`.
pub fn start(ctx: &Ctx, numbers: &[u32]) -> Vec<TaskReport> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = numbers
            .iter()
            .map(|&number| scope.spawn(move || start_one(ctx, number)))
            .collect();
        handles
            .into_iter()
            .zip(numbers)
            .map(|(handle, &number)| {
                handle.join().unwrap_or_else(|_| TaskReport {
                    number,
                    warnings: Vec::new(),
                    result: Err(anyhow::anyhow!(
                        "the task for issue #{number} stopped unexpectedly"
                    )),
                    gates: None,
                })
            })
            .collect()
    })
}

fn start_one(ctx: &Ctx, number: u32) -> TaskReport {
    let mut warnings = Vec::new();
    let mut gates = None;
    let result = (|| {
        let issue = ctx.github.issue(ctx.repo, number)?;
        let mut prepared = prepare(ctx, &issue)?;
        warnings.clone_from(&prepared.warnings);
        let worked = run_worker(ctx, &mut prepared)?;
        // A failed worker is not gated; a timed-out one is (its partial commits count).
        if !matches!(worked.status, RunStatus::Failed(_)) {
            gates = Some(run_gates(
                &ctx.env(),
                &mut prepared,
                "after-worker",
                Tiers::ALL,
            ));
        }
        Ok(worked)
    })();
    TaskReport {
        number,
        warnings,
        result,
        gates,
    }
}

/// Which gate tiers to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tiers {
    pub sandbox: bool,
    pub host: bool,
}

impl Tiers {
    pub const ALL: Self = Self {
        sandbox: true,
        host: true,
    };
    pub const SANDBOX: Self = Self {
        sandbox: true,
        host: false,
    };
    pub const HOST: Self = Self {
        sandbox: false,
        host: true,
    };
}

/// How the gates went.
#[derive(Debug)]
pub struct Gated {
    pub passed: bool,
    /// The first gate that failed, if any.
    pub failed: Option<GateResult>,
    pub outcomes: Vec<GateOutcome>,
}

fn gates_log_entry(phase: &str, outcomes: &[GateOutcome]) -> String {
    if outcomes.is_empty() {
        return format!("== {phase}: no gates configured\n\n");
    }
    let mut log = String::new();
    for outcome in outcomes {
        let result = &outcome.result;
        let exit = result
            .exit
            .map_or_else(|| "no exit code".to_owned(), |code| format!("exit {code}"));
        log.push_str(&format!(
            "== {phase} {}: {}\n{}: {exit}{}\n{}\n\n",
            result.tier,
            result.command,
            if result.passed { "passed" } else { "failed" },
            if outcome.timed_out { ", timed out" } else { "" },
            outcome.output_tail.trim_end(),
        ));
    }
    log
}

/// Spec §5.1 step 5 and §7: runs the gates of the chosen `tiers`. The sandbox tier runs in the
/// worker's sandbox on its workspace; the host tier (only when listed, and only after the
/// sandbox tier passed, unless it was asked for alone) runs on this machine in a clean checkout
/// of the task branch from `repo.git`, which is removed afterwards. The stage is written before
/// anything runs; results go to the record and `gates.log`; the first failure stops a tier.
pub fn run_gates(
    env: &TaskEnv,
    prepared: &mut Prepared,
    phase: &str,
    tiers: Tiers,
) -> Result<Gated> {
    let gates = &env.config.gates;
    let sandbox = prepared
        .record
        .worker
        .as_ref()
        .context("the task has no worker")?
        .sandbox
        .clone();
    let restarted =
        prepared
            .record
            .abandon_interrupted_gates(now(), Process::current(env.probe), env.probe);
    if restarted {
        prepared
            .record
            .notes
            .push("earlier gates were interrupted before they finished".to_owned());
    }
    // What the last passed run covered, and the commit the gates are about to see: a host-only run
    // builds on a sandbox pass of this same commit, and a partial run adds to what was covered.
    let tip = repo::branch_tip(&prepared.meta.join("repo.git"), &prepared.record.branch).ok();
    let covered = prepared
        .record
        .gate_run
        .clone()
        .filter(|run| tip.is_some() && run.commit == tip);
    if tiers.host && !tiers.sandbox && !covered.as_ref().is_some_and(|run| run.sandbox) {
        bail!(
            "the host tier runs only after the sandbox tier passed on the task's current commit; run `sbxm task gates --issue {} --tier sandbox` (or all tiers) first",
            prepared.record.number
        );
    }
    if !restarted {
        prepared
            .record
            .begin_gating(now(), Process::current(env.probe))?;
    }
    record::write(&prepared.meta, &prepared.record)?;

    let mut outcomes = Vec::new();
    let mut sandbox_ok = true;
    if tiers.sandbox {
        let ran = gates::run_sandbox_tier(
            env.backend,
            &sandbox,
            &in_sandbox_path(&prepared.workspace),
            phase,
            &gates.sandbox,
            gates.timeout,
        );
        sandbox_ok = ran.iter().all(|o| o.result.passed);
        outcomes.extend(ran);
    }
    if tiers.host && sandbox_ok && !gates.host.is_empty() {
        outcomes.extend(host_gate_outcomes(env, prepared, phase));
    }

    let run = GateRun {
        sandbox: tiers.sandbox || covered.as_ref().is_some_and(|r| r.sandbox),
        host: tiers.host || covered.as_ref().is_some_and(|r| r.host),
        commit: tip,
    };
    finish_gating(prepared, phase, outcomes, Some(run))
}

/// The host tier (spec §7): a clean checkout of the task branch from `repo.git` in `<id>-gates`
/// (a folder left by a killed run goes first), the host commands run there, and the checkout is
/// removed. A checkout that can't be made is one failed gate, not an error.
fn host_gate_outcomes(env: &TaskEnv, prepared: &mut Prepared, phase: &str) -> Vec<GateOutcome> {
    let gates = &env.config.gates;
    let id = prepared.record.id.clone();
    let checkout = prepared.workspace.with_file_name(format!("{id}-gates"));
    let cleared = if checkout.exists() {
        remove_with_retries(&checkout).map_err(|e| {
            anyhow::anyhow!(
                "could not clear {}, left by an earlier run: {e}; close whatever uses it and \
                 delete it by hand",
                checkout.display()
            )
        })
    } else {
        Ok(())
    };
    match cleared.and_then(|()| {
        repo::clean_checkout(
            &prepared.meta.join("repo.git"),
            &prepared.record.branch,
            &checkout,
        )
    }) {
        Ok(()) => {
            let outcomes =
                gates::run_host_tier(env.host, &checkout, phase, &gates.host, gates.timeout);
            if let Err(e) = remove_with_retries(&checkout) {
                prepared
                    .record
                    .notes
                    .push(format!("could not remove {}: {e}", checkout.display()));
            }
            outcomes
        }
        Err(e) => vec![GateOutcome {
            result: GateResult {
                phase: phase.to_owned(),
                tier: "host".to_owned(),
                command: "(clean checkout)".to_owned(),
                exit: None,
                passed: false,
            },
            output_tail: format!("{e:#}"),
            timed_out: false,
        }],
    }
}

/// Records how the gates went: `gates.log`, the record's results, and the stage's status
/// (`passed` or `gates-failed`), written to disk.
fn finish_gating(
    prepared: &mut Prepared,
    phase: &str,
    outcomes: Vec<GateOutcome>,
    run: Option<GateRun>,
) -> Result<Gated> {
    let failed = outcomes
        .iter()
        .find(|o| !o.result.passed)
        .map(|o| o.result.clone());
    let log_path = prepared.meta.join("gates.log");
    let mut log = fs::read_to_string(&log_path).unwrap_or_default();
    log.push_str(&gates_log_entry(phase, &outcomes));
    // The log is a convenience: failing to write it must not leave the task stuck in `running`.
    if let Err(e) = fs::write(&log_path, log) {
        prepared
            .record
            .notes
            .push(format!("could not write {}: {e}", log_path.display()));
    }
    prepared
        .record
        .gates
        .extend(outcomes.iter().map(|o| o.result.clone()));
    prepared.record.gate_run = run.filter(|_| failed.is_none());
    prepared.record.finish(if failed.is_some() {
        Status::GatesFailed
    } else {
        Status::Passed
    })?;
    record::write(&prepared.meta, &prepared.record)?;
    Ok(Gated {
        passed: failed.is_none(),
        failed,
        outcomes,
    })
}
/// Removes a folder a build may still hold files in for a moment (antivirus, indexers).
fn remove_with_retries(dir: &Path) -> std::io::Result<()> {
    let mut last = Ok(());
    for attempt in 0..5 {
        match fs::remove_dir_all(dir) {
            Ok(()) => return Ok(()),
            Err(e) if !dir.exists() => {
                let _ = e;
                return Ok(());
            }
            Err(e) => last = Err(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(100 << attempt));
    }
    last
}

/// The flag that names a task again on the command line, for a one-line recovery hint: a spec
/// task has no path recorded, so it gets a placeholder (as `task resume`'s own flag does).
fn task_flag(record: &Record) -> String {
    match record.kind {
        Kind::Pr => format!("--pr {}", record.number),
        Kind::Spec => "--spec <file>".to_owned(),
        Kind::Issue => format!("--issue {}", record.number),
    }
}

/// The recorded worker, and the profile its sandbox was built with: `None` only in a record
/// written before issue 118's resume fix (M-2, PR 163 review), which never saved one. Refused with
/// a one-line recovery instead of guessing a profile the task never ran under.
fn worker_profile<'a>(record: &'a Record, worker: &'a Agent) -> Result<&'a str> {
    worker.profile.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "task {}'s record has no saved sandbox profile (it predates issue 118's resume fix); \
             remove it and start it again: `sbxm task rm {}`",
            record.id,
            task_flag(record)
        )
    })
}

/// The recorded worker's resolved CPU and memory: `None` only in a record written before issue
/// 118's resume fix round 2 (M-2, PR 163 review), which saved the optional overrides instead of
/// what the sandbox was actually created with. Refused the same way as a missing profile, instead
/// of substituting today's global defaults for a task that never ran under them.
fn worker_resources(record: &Record, worker: &Agent) -> Result<Resources> {
    match (worker.cpus, worker.memory.clone()) {
        (Some(cpus), Some(memory)) => Ok(Resources { cpus, memory }),
        _ => Err(anyhow::anyhow!(
            "task {}'s record has no saved sandbox resources (it predates issue 118's resume \
             fix); remove it and start it again: `sbxm task rm {}`",
            record.id,
            task_flag(record)
        )),
    }
}

/// What to run instead, once [`check_config_drift`] refuses: an issue task's worker is recreated
/// afresh by `--restart`; a spec task has no such flag, so it is removed and started again from
/// the same file.
fn restart_hint(record: &Record) -> String {
    match record.kind {
        Kind::Issue => format!("sbxm task start --restart --issue {}", record.number),
        _ => format!("sbxm task rm {}", task_flag(record)),
    }
}

/// Refuses before `ensure_worker_sandbox` removes, rebuilds or records anything (M-2, PR 163
/// review round 2): hashes the recorded profile's contents as they are on disk right now, together
/// with the recorded resources and the worker's harness, the same way [`config::config_hash`]
/// hashed them when the task started, and bails if the result no longer matches
/// `Record::config_hash`. An edit to the profile (egress, instructions, setup, secrets, skills) or
/// to the global defaults a task without explicit overrides resolved against must never silently
/// run resumed work in a differently provisioned or differently restricted sandbox.
fn check_config_drift(
    env: &TaskEnv,
    record: &Record,
    profile_name: &str,
    resources: &Resources,
) -> Result<()> {
    let global = GlobalConfig::load(env.config_dir)?;
    let profile = Profile::load(global.profiles_dir(), profile_name)?;
    let harness = env.config.worker.harness;
    let hash = config::config_hash(profile_name, &profile, resources, harness);
    if hash != record.config_hash {
        bail!(
            "task {}'s sandbox config changed since it started; restart it with `{}`, or restore \
             the profile {profile_name:?} to what it was",
            record.id,
            restart_hint(record)
        );
    }
    Ok(())
}

/// Checks before `task resume` restores the worker's input, rebuilds its sandbox, or runs the
/// worker or a fix round again (M-3, PR 163 review): the recorded worker's harness and the
/// profile its sandbox was built with — never the current `sbxm-task.toml`, which `--profile` or
/// an edit could have changed since the task started — must have their secrets stored in `sbx`,
/// the same way `prepare` checks the worker up front. Shares `check_secrets` with `prepare` so the
/// two can't drift.
pub fn check_worker(env: &TaskEnv, prepared: &Prepared) -> Result<()> {
    let worker = prepared
        .record
        .worker
        .as_ref()
        .context("the task has no worker")?;
    let profile_name = worker_profile(&prepared.record, worker)?;
    let global = GlobalConfig::load(env.config_dir)?;
    let profile = Profile::load(global.profiles_dir(), profile_name)?;
    check_secrets(
        env.backend,
        env.config.worker.harness,
        "the worker",
        profile_name,
        &profile,
    )
}

/// Makes the worker's sandbox again from the profile and resources its record saved when it is
/// gone (`task resume`, issue 118; a sandbox can vanish between stages, workflow note G17), over
/// the same clone: the kits are built again under the task's `kits/`. With `replace`, a sandbox
/// that still exists is removed first (a preparation that was cut off may have left one half
/// made), but only once the recorded profile and resources are confirmed and [`check_config_drift`]
/// passes, so a refusal never removes, rebuilds or records anything. Returns whether it was made.
pub(super) fn ensure_worker_sandbox(
    env: &TaskEnv,
    prepared: &Prepared,
    replace: bool,
) -> Result<bool> {
    let worker = prepared
        .record
        .worker
        .as_ref()
        .context("the task has no worker")?;
    let sandbox = worker.sandbox.clone();
    if !replace && env.backend.list()?.iter().any(|s| s.name == sandbox) {
        return Ok(false);
    }
    let profile_name = worker_profile(&prepared.record, worker)?.to_owned();
    let resources = worker_resources(&prepared.record, worker)?;
    check_config_drift(env, &prepared.record, &profile_name, &resources)?;
    if replace {
        let _ = env.backend.remove(&sandbox);
    }
    let harness = env.config.worker.harness;
    let kit_set = kits::build_for(
        env.config_dir,
        &profile_name,
        &[harness],
        &Overrides {
            cpus: Some(resources.cpus),
            memory: Some(resources.memory.clone()),
        },
        &prepared.meta.join("kits"),
        env.backend,
    )?;
    let harness_kits = kit_set
        .get(harness)
        .cloned()
        .context("no kits were built for the worker's harness")?;
    env.backend
        .create(&CreateSpec {
            name: sandbox.clone(),
            agent: harness.agent_arg().into(),
            workspace: prepared.workspace.clone(),
            cpus: kit_set.resources.cpus,
            memory: kit_set.resources.memory.clone(),
            skills: harness_kits.skills_store,
            kits: harness_kits.dirs.clone(),
        })
        .with_context(|| format!("cannot create sandbox {sandbox}"))?;
    Ok(true)
}

/// Whether the last passed gate run covered the sandbox tier and, when one is configured, the host
/// tier, on the task branch's current commit.
fn gates_cover_everything(env: &TaskEnv, prepared: &Prepared) -> bool {
    let tip = repo::branch_tip(&prepared.meta.join("repo.git"), &prepared.record.branch).ok();
    prepared.record.gate_run.as_ref().is_some_and(|run| {
        tip.is_some()
            && run.commit == tip
            && run.sandbox
            && (run.host || env.config.gates.host.is_empty())
    })
}

/// What the worker is told on its command line; the real prompt is a file (decision 131), since
/// a rendered prompt carries the whole issue and can outgrow a command line.
const AGENT_PROMPT: &str =
    "Read the file .sbxm-task/prompt.md in the current directory and follow it exactly.";

/// The biggest `result.md` copied to the host.
const RESULT_CAP: u64 = 1024 * 1024;

/// How the worker's run went and what was collected from it.
#[derive(Debug)]
pub struct Worked {
    pub status: RunStatus,
    /// Also saved in the record.
    pub notes: Vec<String>,
    /// Commits the worker's branch has over the base, after collecting its bundle.
    pub commits: u32,
}

fn run_label(status: &RunStatus) -> String {
    match status {
        RunStatus::Completed => "completed".to_owned(),
        RunStatus::TimedOut => "timed-out".to_owned(),
        RunStatus::Failed(why) => format!("failed: {why}"),
    }
}

/// Spec §5.1 steps 3 and 4: runs the worker, then collects what it left, whatever its status
/// (a timed-out or failed run keeps its partial commits). The stage is written before the agent
/// starts; a write failure stops the command. `Err` means the run couldn't be attempted.
pub fn run_worker(ctx: &Ctx, prepared: &mut Prepared) -> Result<Worked> {
    prepared
        .record
        .worker
        .as_ref()
        .context("the task has no worker")?;
    prepared
        .record
        .advance(Stage::Working, now(), Process::current(ctx.probe))?;
    record::write(&prepared.meta, &prepared.record)?;
    work(&ctx.env(), prepared)
}

/// Writes the task's input (`prompt.md`, and `issue.md` or a spec task's `source.md`) into the
/// worker's clone again from the copies in the task's folder, for a worker that runs again (`task
/// resume`, issue 118): the last run could change or delete the clone's copies. Refuses a control
/// folder or file the agent replaced with a link.
pub(super) fn restore_worker_input(prepared: &Prepared) -> Result<()> {
    let source = match prepared.record.kind {
        Kind::Spec => "source.md",
        _ => "issue.md",
    };
    let read = |name: &str| {
        fs::read(prepared.meta.join(name))
            .with_context(|| format!("the task's {name} is missing from its folder"))
    };
    let (prompt, text) = (read("worker-prompt.md")?, read(source)?);
    repo::write_agent_files(
        &prepared.workspace,
        &[(source, &text), ("prompt.md", &prompt)],
        Existing::Refuse,
    )
}

/// The worker's run itself, in a task already at `working/running` (written): the headless agent,
/// its transcript and status, then what it left collected. Shared by [`run_worker`] and `task
/// resume`, which runs it again in the same clone (issue 118).
pub(super) fn work(env: &TaskEnv, prepared: &mut Prepared) -> Result<Worked> {
    let worker = &env.config.worker;
    let sandbox = prepared
        .record
        .worker
        .as_ref()
        .context("the task has no worker")?
        .sandbox
        .clone();

    let started = Instant::now();
    let opts = HeadlessOpts {
        model: worker.model.clone(),
        high_effort: false,
        budget_usd: None,
        is_git_repo: true,
    };
    let result = match headless::run(
        env.backend,
        &sandbox,
        &in_sandbox_path(&prepared.workspace),
        worker.harness,
        AGENT_PROMPT,
        &opts,
        worker.time_limit,
    ) {
        Ok(result) => result,
        Err(e) => {
            prepared.record.finish(Status::Failed)?;
            record::write(&prepared.meta, &prepared.record)?;
            return Err(e.context(format!("cannot run the worker in {sandbox}")));
        }
    };

    let transcripts = prepared.meta.join("transcripts");
    fs::create_dir_all(&transcripts)?;
    fs::write(transcripts.join("worker.jsonl"), &result.transcript)?;
    if let Some(agent) = prepared.record.worker.as_mut() {
        agent.run = Some(RunInfo {
            status: run_label(&result.status),
            usage: serde_json::json!({
                "input_tokens": result.usage.input_tokens,
                "output_tokens": result.usage.output_tokens,
                "cost_usd": result.usage.cost_usd,
            }),
            duration_s: started.elapsed().as_secs(),
        });
    }
    let mut status = result.status;
    prepared.record.finish(match &status {
        RunStatus::Completed => Status::Completed,
        RunStatus::TimedOut => Status::TimedOut,
        RunStatus::Failed(_) => Status::Failed,
    })?;
    record::write(&prepared.meta, &prepared.record)?;

    let mut notes = Vec::new();
    let commits = collect(env.backend, prepared, &sandbox, &mut status, &mut notes)?;
    prepared.record.notes.extend(notes.iter().cloned());
    record::write(&prepared.meta, &prepared.record)?;
    Ok(Worked {
        status,
        notes,
        commits,
    })
}

/// What the bundle step found in the worker's clone.
enum Bundled {
    /// The branch's commits ahead of the base now in `repo.git`.
    Commits(u32),
    /// Nothing ahead of the base.
    Nothing,
}

/// The fixed bundle command runs in the sandbox; the bundle is verified and fetched into
/// `repo.git` on the host. Nothing runs git in the agent's clone here (decision 159).
fn bundle_and_fetch(
    backend: &dyn SandboxBackend,
    prepared: &Prepared,
    sandbox: &str,
) -> Result<Bundled> {
    let branch = prepared.record.branch.clone();
    let ws = in_sandbox_path(&prepared.workspace)
        .to_string_lossy()
        .into_owned();
    let origin = prepared.record.bundle_exclusion();
    let out = backend.exec(
        sandbox,
        &ExecSpec {
            workdir: None,
            argv: [
                "git",
                "-C",
                &ws,
                "bundle",
                "create",
                ".sbxm-task/branch.bundle",
                &branch,
                &origin,
            ]
            .map(str::to_owned)
            .into(),
            stdin: Stdin::Closed,
        },
    )?;
    if out.exit_code == Some(0) {
        let repo_git = prepared.meta.join("repo.git");
        repo::fetch_bundle(&repo_git, &prepared.workspace, &branch, BUNDLE_CAP)?;
        Ok(Bundled::Commits(prepared.record.commits_ahead(&repo_git)?))
    } else if out.stderr.contains("empty bundle") {
        Ok(Bundled::Nothing)
    } else {
        bail!(
            "`git bundle create` failed in the sandbox: {}",
            out.stderr.trim()
        )
    }
}

/// Before gates run on demand: whatever is committed in the worker's clone now (a fix made by
/// hand after a failed gate, say) is collected into `repo.git`, because the host tier, the
/// reviewer's checkout and `finish` all read `repo.git`, never the clone. Without it a task could
/// pass its gates on one revision and be reviewed and published on an older one.
pub fn recollect_commits(env: &TaskEnv, prepared: &Prepared) -> Result<()> {
    let sandbox = prepared
        .record
        .worker
        .as_ref()
        .context("the task has no worker")?
        .sandbox
        .clone();
    bundle_and_fetch(env.backend, prepared, &sandbox).context(concat!(
        "cannot collect the commits in the worker's clone before the gates; ",
        "the gates would not be testing what review and finish will use"
    ))?;
    Ok(())
}

/// Brings the worker's commits, `result.md` and a note about unsaved work to the host. A failure
/// to collect marks the worker failed (with a note); it is not an `Err`.
fn collect(
    backend: &dyn SandboxBackend,
    prepared: &mut Prepared,
    sandbox: &str,
    status: &mut RunStatus,
    notes: &mut Vec<String>,
) -> Result<u32> {
    let ws = in_sandbox_path(&prepared.workspace)
        .to_string_lossy()
        .into_owned();
    let in_sandbox = |argv: &[&str]| ExecSpec {
        workdir: None,
        argv: argv.iter().map(|a| (*a).to_owned()).collect(),
        stdin: Stdin::Closed,
    };

    let mut commits = 0;
    match bundle_and_fetch(backend, prepared, sandbox) {
        Ok(Bundled::Commits(n)) => commits = n,
        Ok(Bundled::Nothing) => notes.push("the worker made no commits".to_owned()),
        Err(e) => {
            let why = format!("{e:#}");
            notes.push(format!("could not collect the worker's commits: {why}"));
            *status = RunStatus::Failed(why);
            prepared.record.status = Status::Failed;
        }
    }

    // Work the agent left uncommitted is not collected: say so (the agent may have ended early).
    if let Ok(out) = backend.exec(
        sandbox,
        &in_sandbox(&["git", "-C", &ws, "status", "--porcelain"]),
    ) {
        let changed: Vec<&str> = out
            .stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if out.exit_code == Some(0) && !changed.is_empty() {
            let shown = changed
                .iter()
                .take(5)
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
            let more = if changed.len() > 5 { ", ..." } else { "" };
            notes.push(format!(
                "{} uncommitted change(s) in the worker's clone were not collected \
                 ({shown}{more}); the agent may have ended early",
                changed.len()
            ));
        }
    }

    match repo::read_agent_file(&prepared.workspace, "result.md", RESULT_CAP) {
        Ok(AgentFile::Text(text)) => fs::write(prepared.meta.join("result.md"), text)?,
        Ok(AgentFile::Missing) => notes.push("the worker wrote no result.md".to_owned()),
        Ok(AgentFile::TooLarge(_)) => notes.push(
            "result.md is larger than 1 MiB and was not copied; read it in the worker's clone"
                .to_owned(),
        ),
        Err(e) => notes.push(format!("result.md was refused: {e:#}")),
    }
    Ok(commits)
}

/// The biggest `review.md` read from the reviewer's clone.
const REVIEW_CAP: u64 = 1024 * 1024;

/// What the worker is told for the fix round; the prompt itself is a file.
const FIX_PROMPT: &str =
    "Read the file .sbxm-task/fix-prompt.md in the current directory and follow it exactly.";

/// Whether a review may start now (`task review`). Decided through `machine::TABLE`; the reason
/// for a refusal says what to do.
pub fn check_can_review(task: &Record, probe: &dyn ProcessProbe) -> Result<()> {
    let id = &task.id;
    // A spec task has no issue number: its status is read without a filter.
    let status_hint = match task.kind {
        record::Kind::Spec => "sbxm task status".to_owned(),
        _ => format!("sbxm task status --issue {}", task.number),
    };
    // What continues a failed or interrupted stage (issue 118).
    let resume = match task.kind {
        record::Kind::Spec => "sbxm task resume --spec <file>".to_owned(),
        _ => format!("sbxm task resume --issue {}", task.number),
    };
    let state = machine::State {
        stage: task.stage,
        status: task.status,
        interrupted: task.is_interrupted(probe),
        kind: task.kind,
    };
    if machine::verdict(&state, machine::Event::Review) {
        return Ok(());
    }
    match (task.stage, task.status) {
        (Stage::Working | Stage::Fixing, Status::Running) => {
            if task.is_interrupted(probe) {
                bail!(
                    "task {id} was interrupted while its {} stage was running; continue it with \
                     `{resume}`, or see `{status_hint}`",
                    task.stage.name()
                )
            }
            bail!(
                "the worker is still running for task {id} (worker still running); wait for `sbxm task start` to finish"
            )
        }
        (Stage::Working | Stage::Fixing, _) => bail!(
            "the worker failed for task {id}, so there is nothing to review; run it again with \
             `{resume}`, or see `{status_hint}`"
        ),
        (Stage::Gating, _) => {
            bail!("gates are running for task {id}, or were interrupted; see `{status_hint}`")
        }
        (Stage::Reviewing, _) => bail!(
            "task {id} is already being reviewed or has been (stage reviewing); if the sbxm \
             process that ran it is gone, continue it with `{resume}`, or see `{status_hint}`"
        ),
        (Stage::Prepared, _) if task.kind == record::Kind::Spec => bail!(
            "no worker has run for task {id}; see `{status_hint}`, and remove its folders under the base dir to start it again"
        ),
        (Stage::Prepared, _) => {
            bail!("no worker has run for task {id}; run `sbxm task start` first")
        }
        (stage @ (Stage::Ready | Stage::Finished), _) => bail!(
            "task {id} is already {}; the review is review.md in its folder",
            stage.name()
        ),
    }
}

/// How a whole review went.
#[derive(Debug, Default)]
pub struct ReviewReport {
    /// One entry per reviewer round that finished.
    pub rounds: Vec<Reviewed>,
    /// Whether at least one fix round ran.
    pub fix_ran: bool,
    /// Set when the round budget ran out with a gate still failing: the review stops there.
    pub gates_failed: Option<GateResult>,
    /// Must-fix findings in the last review (reported, not an error; `0` unless the round budget
    /// ran out too, see `Record::stopped`).
    pub must_fix_left: u32,
    pub warnings: Vec<String>,
}

/// Runs the gates, feeding a fix round for the gate's own output on a failure and retrying, until
/// they pass or the round budget (`Record::fix_rounds`) is used (issue 117, decision 173(d)).
/// `Ok(None)` once they pass; `Ok(Some(failed))` when the budget ran out with `failed` still
/// failing (the review stops there, as it always did for a single round).
fn pass_gates(
    env: &TaskEnv,
    prepared: &mut Prepared,
    report: &mut ReviewReport,
    phase: &str,
) -> Result<Option<GateResult>> {
    loop {
        let gated = run_gates(env, prepared, phase, Tiers::ALL)?;
        let Some(failed) = gated.failed else {
            return Ok(None);
        };
        if prepared.record.round >= prepared.record.fix_rounds {
            return Ok(Some(failed));
        }
        let outcome = gated
            .outcomes
            .iter()
            .find(|o| !o.result.passed)
            .context("a failed gate run has no failing outcome")?;
        run_fix_round(env, prepared, Some(outcome))?;
        prepared.record.round += 1;
        report.fix_ran = true;
    }
}

/// Spec §5.2, issue tasks (issue 117, decisions 173(a)(d)(e), 174): checks first (nothing is
/// created if a secret is missing), gates when they haven't passed since the last change, then a
/// reviewer. A review with must-fix findings gets a fix round by the worker, as long as the round
/// budget (`[worker] fix_rounds`) isn't used: its commits are collected, the gates run again (also
/// feeding a fix round on their own failure), and the review runs again, scoped to only the
/// commits since the last one. Once a narrow review finds nothing, one more, full review runs
/// before the task is `ready`, so nothing a narrow scope missed stands in the way. The task never
/// ends `ready` with must-fix findings while rounds are left; when the budget runs out first, it
/// ends `ready` with `Record::stopped` set to `rounds-exhausted`.
pub fn review_issue(env: &TaskEnv, prepared: &mut Prepared) -> Result<ReviewReport> {
    check_can_review(&prepared.record, env.probe)?;
    let mut report = ReviewReport {
        warnings: check_reviewer(env, prepared)?,
        ..ReviewReport::default()
    };
    // Refused before the gates run when no review round number is left (worked out again after
    // them, as their fix rounds count).
    next_review_round(prepared)?;

    let gates_current = match (prepared.record.stage, prepared.record.status) {
        // Passed gates count only if they covered every configured tier on the branch as it is now.
        (Stage::Gating, Status::Passed) => gates_cover_everything(env, prepared),
        // A review that failed already had its gates pass; they ran for the same commits.
        (Stage::Reviewing, Status::Failed) => true,
        _ => false,
    };
    if !gates_current && let Some(failed) = pass_gates(env, prepared, &mut report, "before-review")?
    {
        report.gates_failed = Some(failed);
        return Ok(report);
    }

    let round = next_review_round(prepared)?;
    review_rounds(env, prepared, report, round, None, false)
}

/// The number for a review round that is about to start: after the last recorded review and
/// every saved `review-<n>.md`, and never below the fix rounds counted. Review rounds outnumber fix
/// rounds (a clean narrow review is followed by a full one), and a record older than
/// `Record::review` (issue 118) only has the files to go by: a number taken from the fix-round
/// counter alone would overwrite an earlier round's saved review.
fn next_review_round(prepared: &Prepared) -> Result<u32> {
    let saved = review::saved_rounds(&prepared.meta)
        .last()
        .copied()
        .unwrap_or(0);
    let recorded = prepared.record.review.as_ref().map_or(0, |last| last.round);
    let from_record = prepared.record.round.max(recorded);
    // Moving files out cannot lower the maximum the record itself holds: only a saved file
    // strictly above both record fields is freed by moving it out (issue 162).
    review_round_after(prepared, from_record.max(saved), saved > from_record)
}

/// The review round after `round`, or an error once the numbers run out: a wrapped number would
/// reuse, and overwrite, a saved `review-<n>.md` (decision 177(o)). `moving_helps` must be set
/// only when a saved `review-<n>.md` file is the sole reason `round` is exhausted: moving it out
/// of the task's folder is then a recovery that works on the next attempt. `record.round` or
/// `record.review.round` reaching `u32::MAX` is never freed by moving any file, so the hint must
/// not claim it is (issue 162).
fn review_round_after(prepared: &Prepared, round: u32, moving_helps: bool) -> Result<u32> {
    round.checked_add(1).with_context(|| {
        if moving_helps {
            format!(
                "task {} has no review round number left after {round}; move the review-<n>.md files out of {} or remove the task with `sbxm task rm`",
                prepared.record.id,
                prepared.meta.display()
            )
        } else {
            format!(
                "task {} has no review round number left after {round}; remove the task with `sbxm task rm`",
                prepared.record.id
            )
        }
    })
}

/// Refuses a recorded review result that [`review_rounds`] would follow with another review
/// round (a clean narrow review, or findings with fix-round budget left and no repeat) when no
/// review round number is left after it, so `task resume` refuses before it touches a sandbox,
/// runs a fix round or writes anything (issue 161).
pub(super) fn check_replay(prepared: &Prepared, result: &record::ReviewResult) -> Result<()> {
    let another_review = if result.must_fix == 0 {
        !result.full
    } else {
        !result.repeat && prepared.record.round < prepared.record.fix_rounds
    };
    if another_review {
        // `result.round` came from the record, not freshly read from `review-<n>.md` files.
        review_round_after(prepared, result.round, false)?;
    }
    Ok(())
}

/// The review rounds of [`review_issue`], from review round `round` on: each review, and while
/// must-fix findings are left and the budget allows, a fix round and the gates before the next;
/// then `ready`. `first` is a review that already completed (`task resume` replaying the recorded
/// [`record::ReviewResult`], issue 118): it stands in for the first round instead of running the
/// reviewer again (it was reported when it ran, so it is not one of `report.rounds`), and the
/// rounds go on from its number. With `ignore_repeat`, the first review that runs does not stop
/// the task on a repeat: the first review after `task resume` reopened a stopped task (spec
/// §5.3), so it is not stopped again at once.
pub(super) fn review_rounds(
    env: &TaskEnv,
    prepared: &mut Prepared,
    mut report: ReviewReport,
    mut round: u32,
    mut first: Option<record::ReviewResult>,
    mut ignore_repeat: bool,
) -> Result<ReviewReport> {
    loop {
        let (must_fix, full, repeat) = match first.take() {
            Some(replayed) => {
                round = replayed.round;
                (replayed.must_fix, replayed.full, replayed.repeat)
            }
            None => {
                let reviewed = run_reviewer(env, prepared, round)?;
                let repeat = reviewed.repeat && !std::mem::take(&mut ignore_repeat);
                let outcome = (reviewed.must_fix, reviewed.full, repeat);
                report.warnings.extend(reviewed.warnings.clone());
                report.rounds.push(reviewed);
                outcome
            }
        };
        report.must_fix_left = must_fix;

        if must_fix == 0 {
            if full {
                break;
            }
            // A clean narrow review proves nothing about what it didn't see: one more, full
            // review runs before the task is ready (spec §5.2). `round` is already in memory,
            // not freshly read from files, so moving one would not free it.
            round = review_round_after(prepared, round, false)?;
            continue;
        }
        // The no-progress rule (spec §5.3, issue 119): a validated repeat stops the task even
        // with rounds left in the budget, before the rounds-exhausted check below.
        if repeat {
            prepared.record.stopped = Some(record::Stopped::RepeatFinding);
            break;
        }
        if prepared.record.round >= prepared.record.fix_rounds {
            prepared.record.stopped = Some(record::Stopped::RoundsExhausted);
            break;
        }

        // The review after the fix needs a number: refused before the fix round and the gates
        // change anything (issue 161). `round` is already in memory, not freshly read from
        // files, so moving one would not free it.
        let next = review_round_after(prepared, round, false)?;
        run_fix_round(env, prepared, None)?;
        prepared.record.round += 1;
        report.fix_ran = true;
        if let Some(failed) = pass_gates(env, prepared, &mut report, "after-fix")? {
            report.gates_failed = Some(failed);
            return Ok(report);
        }
        round = next;
    }

    prepared
        .record
        .advance(Stage::Ready, now(), Process::current(env.probe))?;
    record::write(&prepared.meta, &prepared.record)?;
    Ok(report)
}

/// After a fix round that `task resume` ran again (issue 118): the round is counted (a failed
/// round never was), the gates run (feeding further fix rounds while the budget allows), then
/// the review rounds go on from the one after the last recorded review, to `ready`.
pub(super) fn continue_after_fix(env: &TaskEnv, prepared: &mut Prepared) -> Result<ReviewReport> {
    after_fix(env, prepared, false)
}

/// A task that stopped `ready` with must-fix findings, reopened by `task resume` (issue 118,
/// decisions 173(c), 177(b)(q)): a fix round from the recorded review (its `input`, read and
/// placed before anything changed), then as after any fix round, except that the first review
/// after it doesn't stop the task on a repeat.
pub(super) fn fix_from_ready(
    env: &TaskEnv,
    prepared: &mut Prepared,
    input: &FixInput,
) -> Result<ReviewReport> {
    start_fix_round(env, prepared, input)?;
    after_fix(env, prepared, true)
}

fn after_fix(env: &TaskEnv, prepared: &mut Prepared, ignore_repeat: bool) -> Result<ReviewReport> {
    let mut report = ReviewReport::default();
    prepared.record.round += 1;
    report.fix_ran = true;
    if let Some(failed) = pass_gates(env, prepared, &mut report, "after-fix")? {
        report.gates_failed = Some(failed);
        return Ok(report);
    }
    let round = next_review_round(prepared)?;
    review_rounds(env, prepared, report, round, None, ignore_repeat)
}

/// How the review of a pull request went.
#[derive(Debug, Default)]
pub struct PrReview {
    /// `None` when the gates failed first.
    pub reviewed: Option<Reviewed>,
    pub gates_failed: Option<GateResult>,
    /// Whether the review was posted as a comment on the PR.
    pub posted: bool,
    pub warnings: Vec<String>,
}

/// Whether a pull request's review may start (or be tried again) now.
pub fn check_can_review_pr(task: &Record) -> Result<()> {
    let (id, number) = (&task.id, task.number);
    let state = machine::State {
        stage: task.stage,
        status: task.status,
        // This check never sees a probe (no command retries an interrupted PR gating run yet;
        // see `sdlc/spikes/state-table.md` dead end T3/T9 for PR tasks).
        interrupted: false,
        kind: task.kind,
    };
    if machine::verdict(&state, machine::Event::Review) {
        return Ok(());
    }
    match (task.stage, task.status) {
        (stage @ (Stage::Ready | Stage::Finished), _) => bail!(
            "task {id} is already {}; its review is review.md in the task folder; to review the PR \
             again after it changed, run `sbxm task rm --pr {number}` first",
            stage.name()
        ),
        (stage, status) => bail!(
            "task {id} is at stage {} ({}); see `sbxm task status --pr {number}`",
            stage.name(),
            status.name()
        ),
    }
}

/// Spec §5.2 "PR": the gates run on a clean checkout of the PR's head (the sandbox tier inside the
/// reviewer's own sandbox, since a PR has no worker; the host tier as usual), then the reviewer
/// runs once (no fix round), and a valid review is posted as a PR comment. The reviewer's sandbox
/// and clone are removed on every path. A failed gate stops before the reviewer; an invalid review
/// is never posted; a failed post keeps the saved review and says how to post it by hand.
pub fn review_pr(
    env: &TaskEnv,
    github: &dyn GitHubBackend,
    prepared: &mut Prepared,
) -> Result<PrReview> {
    check_can_review_pr(&prepared.record)?;
    let mut report = PrReview {
        warnings: check_reviewer(env, prepared)?,
        ..PrReview::default()
    };
    let id = prepared.record.id.clone();
    let reviewer = &env.config.reviewer;
    let gates = &env.config.gates;
    let clone = prepared.workspace.with_file_name(format!("{id}-review"));
    let sandbox = format!("sbxm-task-{id}-review-{}", reviewer.harness.as_str());
    let _ = fs::remove_file(prepared.meta.join("review.md"));

    // Gating: in the reviewer's sandbox, over a clean clone of the PR's head.
    if prepared.record.stage != Stage::Reviewing {
        prepared
            .record
            .begin_gating(now(), Process::current(env.probe))?;
        record::write(&prepared.meta, &prepared.record)?;
    }
    let gated_or_ready = (|| -> Result<Option<Gated>> {
        if prepared.record.stage == Stage::Reviewing {
            // A review that failed is tried again; its gates passed before.
            prepared
                .record
                .begin_review(now(), Process::current(env.probe))?;
            record::write(&prepared.meta, &prepared.record)?;
            open_review_workspace(env, prepared, 1, &clone, &sandbox, &[])?;
            return Ok(None);
        }
        open_review_workspace(env, prepared, 1, &clone, &sandbox, &[])?;
        let mut outcomes = gates::run_sandbox_tier(
            env.backend,
            &sandbox,
            &in_sandbox_path(&clone),
            "pr",
            &gates.sandbox,
            gates.timeout,
        );
        if outcomes.iter().all(|o| o.result.passed) && !gates.host.is_empty() {
            outcomes.extend(host_gate_outcomes(env, prepared, "pr"));
        }
        Ok(Some(finish_gating(prepared, "pr", outcomes, None)?))
    })();
    let cleanup = |prepared: &mut Prepared| {
        let _ = env.backend.remove(&sandbox);
        if let Err(e) = remove_with_retries(&clone) {
            prepared
                .record
                .notes
                .push(format!("could not remove {}: {e}", clone.display()));
        }
    };
    match gated_or_ready {
        Err(e) => {
            cleanup(prepared);
            prepared
                .record
                .notes
                .push(format!("could not start the review: {e:#}"));
            // Whichever stage was running did not finish.
            let failed = if prepared.record.stage == Stage::Gating {
                Status::GatesFailed
            } else {
                Status::Failed
            };
            prepared.record.finish(failed)?;
            record::write(&prepared.meta, &prepared.record)?;
            return Err(e);
        }
        Ok(Some(gated)) if !gated.passed => {
            cleanup(prepared);
            record::write(&prepared.meta, &prepared.record)?;
            report.gates_failed = gated.failed;
            return Ok(report);
        }
        Ok(Some(_)) => {
            prepared
                .record
                .begin_review(now(), Process::current(env.probe))?;
            record::write(&prepared.meta, &prepared.record)?;
        }
        Ok(None) => {}
    }

    // The reviewer, once: a PR task is always a single full review (it has no worker to fix
    // anything and so never narrows a later one, and so never repeats one either).
    let ran = run_review_agent(env, prepared, 1, &clone, &sandbox, &[]);
    cleanup(prepared);
    let outcome = match ran {
        Ok(outcome) => outcome,
        Err(e) => {
            prepared.record.notes.push(format!("review: {e:#}"));
            prepared.record.finish(Status::Failed)?;
            record::write(&prepared.meta, &prepared.record)?;
            return Err(e);
        }
    };
    let must_fix = outcome.must_fix;
    let (risk, risk_reasons) = (outcome.risk, outcome.risk_reasons.clone());
    prepared.record.open_findings = Some(outcome.open);
    prepared.record.review = Some(record::ReviewResult {
        round: 1,
        must_fix,
        full: true,
        repeat: false,
        risk: outcome.risk,
        risk_reasons: outcome.risk_reasons,
    });
    prepared.record.finish(Status::Completed)?;
    prepared
        .record
        .advance(Stage::Ready, now(), Process::current(env.probe))?;
    record::write(&prepared.meta, &prepared.record)?;
    report.reviewed = Some(Reviewed {
        round: 1,
        must_fix,
        full: true,
        repeat: false,
        warnings: outcome.warnings,
        risk,
    });

    let review_path = prepared.meta.join("review.md");
    let text = fs::read_to_string(&review_path)?;
    let (repo, number) = (prepared.record.repo.clone(), prepared.record.number);
    let risk_section = risk::render_section(risk, &risk_reasons);
    let comment = review::pr_comment(number, &risk_section, &text);
    match github.pr_comment(&repo, number, &comment) {
        Ok(()) => {
            report.posted = true;
            prepared
                .record
                .notes
                .push(format!("the review was posted on PR #{number}"));
            record::write(&prepared.meta, &prepared.record)?;
            Ok(report)
        }
        Err(e) => {
            prepared
                .record
                .notes
                .push(format!("the review was not posted: {e:#}"));
            record::write(&prepared.meta, &prepared.record)?;
            Err(e.context(format!(
                "the review is saved in {} but could not be posted on PR #{number}; post it \
                 yourself with `gh pr comment {number} --body-file {}`",
                review_path.display(),
                review_path.display()
            )))
        }
    }
}

/// The text written as `.sbxm-task/gate-failure.md`: the failing command, its tier and exit
/// status, and the end of its output, so the fix prompt can point at it (issue 117, decision
/// 173(d)).
fn gate_failure_text(outcome: &GateOutcome) -> String {
    let result = &outcome.result;
    let exit = result
        .exit
        .map_or_else(|| "no exit code".to_owned(), |code| format!("exit {code}"));
    format!(
        "Gate failed: `{}` ({}, {exit}{})\n\n{}\n",
        result.command,
        result.tier,
        if outcome.timed_out { ", timed out" } else { "" },
        outcome.output_tail.trim_end(),
    )
}

/// A fix round (spec §5.2, issue 117): the worker, in its own sandbox, gets either the review or
/// a failed gate's own output and a fix prompt; its new commits are collected like the first
/// time. A failed run, or commits that can't be collected, stop the review (the task is
/// `fixing/failed`). Counts against `Record::fix_rounds`; the caller increments `round`.
fn run_fix_round(
    env: &TaskEnv,
    prepared: &mut Prepared,
    gate_failure: Option<&GateOutcome>,
) -> Result<()> {
    let input = fix_input(env, prepared, gate_failure)?;
    start_fix_round(env, prepared, &input)
}

/// What a fix round is given: the review or a gate's output, as the file the prompt names, and
/// the rendered prompt. Read and rendered before the round changes anything, so a missing review
/// stops it first (issue 118).
pub(super) struct FixInput {
    extra_name: &'static str,
    extra_text: String,
    prompt: String,
}

impl FixInput {
    /// Writes the input and the prompt into the worker's clone, refusing a link the agent left.
    pub(super) fn place(&self, workspace: &Path) -> Result<()> {
        repo::write_agent_files(
            workspace,
            &[
                (self.extra_name, self.extra_text.as_bytes()),
                ("fix-prompt.md", self.prompt.as_bytes()),
            ],
            Existing::Refuse,
        )
    }
}

/// A fix round's [`FixInput`]: the review in the task's folder, or `gate_failure`'s output.
pub(super) fn fix_input(
    env: &TaskEnv,
    prepared: &Prepared,
    gate_failure: Option<&GateOutcome>,
) -> Result<FixInput> {
    prepared
        .record
        .worker
        .as_ref()
        .context("the task has no worker")?;
    let is_spec = prepared.record.kind == Kind::Spec;
    let (role, extra_name, extra_text) = match gate_failure {
        None => (
            if is_spec { Role::FixSpec } else { Role::Fix },
            "review.md",
            fs::read_to_string(prepared.meta.join("review.md"))
                .context("the review to fix is missing; run the review again")?,
        ),
        Some(outcome) => (
            if is_spec {
                Role::FixGateSpec
            } else {
                Role::FixGate
            },
            "gate-failure.md",
            gate_failure_text(outcome),
        ),
    };
    let template = prompts::template(role, &env.config.prompts)?;
    let prompt = prompts::render(
        &template.name,
        &template.text,
        &[
            ("issue", ""),
            ("number", &prepared.record.number.to_string()),
            ("branch", &prepared.record.branch),
            ("base", &prepared.record.base),
            ("repo", &prepared.record.repo),
            ("gates_sandbox", &bullets(&env.config.gates.sandbox)),
            ("gates_host", &bullets(&env.config.gates.host)),
            ("review_path", ".sbxm-task/review.md"),
            ("previous_review_path", ".sbxm-task/previous-review.md"),
            ("gate_output_path", ".sbxm-task/gate-failure.md"),
        ],
    )?;
    Ok(FixInput {
        extra_name,
        extra_text,
        prompt,
    })
}

/// Starts a fix round with its input: the task goes to `fixing/running`, the input is kept in the
/// task's folder and written into the clone, and the worker runs.
pub(super) fn start_fix_round(
    env: &TaskEnv,
    prepared: &mut Prepared,
    input: &FixInput,
) -> Result<()> {
    prepared
        .record
        .advance(Stage::Fixing, now(), Process::current(env.probe))?;
    record::write(&prepared.meta, &prepared.record)?;

    // The round's input is kept in the task's folder too: a gate's output as `gate-failure.md`
    // (a review's is `review.md` already), so `task resume` can run the round again with it.
    let gate_copy = prepared.meta.join("gate-failure.md");
    if input.extra_name == "gate-failure.md" {
        fs::write(&gate_copy, &input.extra_text)?;
    } else {
        let _ = fs::remove_file(&gate_copy);
    }
    input.place(&prepared.workspace)?;
    fs::write(prepared.meta.join("fix-prompt.md"), &input.prompt)?;
    fix(env, prepared)
}

/// A fix round that failed, or whose process was gone while it ran, runs again (`task resume`,
/// issue 118) in the same clone, which keeps what the last run did: the prompt and the input it
/// was given (the review, or the gate's output) are written into the clone again from the task's
/// folder, and the worker runs. Like [`run_fix_round`], the caller counts the round.
pub(super) fn rerun_fix_round(env: &TaskEnv, prepared: &mut Prepared) -> Result<()> {
    let prompt = fs::read_to_string(prepared.meta.join("fix-prompt.md"))
        .context("the failed fix round's prompt (fix-prompt.md) is missing from the task folder")?;
    let gate_copy = prepared.meta.join("gate-failure.md");
    let (extra_name, extra_text) = if gate_copy.is_file() {
        ("gate-failure.md", fs::read_to_string(&gate_copy)?)
    } else {
        (
            "review.md",
            fs::read_to_string(prepared.meta.join("review.md"))
                .context("the review to fix is missing; run the review again")?,
        )
    };
    // The clone is agent-controlled, so it is validated and the input placed in it before the
    // record says the retry is running (as `restore_worker_input` and the stopped-ready fix path
    // already do): a link the failed worker left must be refused with nothing recorded yet.
    repo::write_agent_files(
        &prepared.workspace,
        &[
            (extra_name, extra_text.as_bytes()),
            ("fix-prompt.md", prompt.as_bytes()),
        ],
        Existing::Refuse,
    )?;
    prepared.record.rerun(now(), Process::current(env.probe))?;
    record::write(&prepared.meta, &prepared.record)?;
    fix(env, prepared)
}

/// The fix round's run itself, in a task at `fixing/running` whose prompt is in the clone: the
/// worker, its transcript and status, then its commits collected.
fn fix(env: &TaskEnv, prepared: &mut Prepared) -> Result<()> {
    let worker = &env.config.worker;
    let sandbox = prepared
        .record
        .worker
        .as_ref()
        .context("the task has no worker")?
        .sandbox
        .clone();
    let result = match headless::run(
        env.backend,
        &sandbox,
        &in_sandbox_path(&prepared.workspace),
        worker.harness,
        FIX_PROMPT,
        &HeadlessOpts {
            model: worker.model.clone(),
            high_effort: false,
            budget_usd: None,
            is_git_repo: true,
        },
        worker.time_limit,
    ) {
        Ok(result) => result,
        Err(e) => {
            prepared.record.finish(Status::Failed)?;
            record::write(&prepared.meta, &prepared.record)?;
            return Err(e.context(format!("cannot run the fix round in {sandbox}")));
        }
    };
    let transcripts = prepared.meta.join("transcripts");
    fs::create_dir_all(&transcripts)?;
    fs::write(transcripts.join("fix.jsonl"), &result.transcript)?;
    let mut status = result.status;
    prepared.record.finish(match &status {
        RunStatus::Completed => Status::Completed,
        RunStatus::TimedOut => Status::TimedOut,
        RunStatus::Failed(_) => Status::Failed,
    })?;
    record::write(&prepared.meta, &prepared.record)?;
    if let RunStatus::Failed(why) = &status {
        bail!("the fix round failed ({why}); the worker's clone keeps whatever it did");
    }

    let mut notes = Vec::new();
    collect(env.backend, prepared, &sandbox, &mut status, &mut notes)?;
    prepared
        .record
        .notes
        .extend(notes.iter().map(|n| format!("fix round: {n}")));
    record::write(&prepared.meta, &prepared.record)?;
    if let RunStatus::Failed(why) = status {
        bail!("the fix round's commits could not be collected ({why})");
    }
    Ok(())
}

/// What a reviewer round found.
#[derive(Debug)]
pub struct Reviewed {
    pub round: u32,
    pub must_fix: u32,
    /// Whether this round covered every commit since the task's base (`true`), or only those
    /// since the last review (`false`, spec §5.2, issue 117).
    pub full: bool,
    /// Whether a must-fix finding validly claimed `Repeat of:` a must-fix finding of any review
    /// round before this one (spec §5.3, decisions 174(b), 177(o)): always `false` for round 1,
    /// which has no earlier round to repeat.
    pub repeat: bool,
    /// One warning per must-fix finding whose `Repeat of:` claim was invalid (decision 177(e)):
    /// an id no earlier round has a must-fix finding for, or one in a different file.
    pub warnings: Vec<String>,
    /// The risk level (decision 176): the reviewer's own `Risk:` line, raised to the path-rule
    /// floor but never lowered; `Level::Unknown` for a review with no readable `Risk:` line.
    pub risk: risk::Level,
}

/// Checks before a review creates anything (spec §5.2): the reviewer's provider secret and the
/// profile's secrets are stored, and the profile loads. Returns the warnings to print.
pub fn check_reviewer(env: &TaskEnv, _task: &Prepared) -> Result<Vec<String>> {
    check_reviewer_inputs(env.config_dir, env.backend, env.config)
}

/// The checks of [`check_reviewer`] without a task, so `task run` can make them before its worker
/// has created anything.
pub fn check_reviewer_inputs(
    config_dir: &Path,
    backend: &dyn SandboxBackend,
    config: &TaskConfig,
) -> Result<Vec<String>> {
    let global = GlobalConfig::load(config_dir)?;
    let reviewer = &config.reviewer;
    let profile = Profile::load(global.profiles_dir(), &config.sandbox.profile)?;
    check_secrets(
        backend,
        reviewer.harness,
        "the reviewer",
        &config.sandbox.profile,
        &profile,
    )?;
    let mut warnings = config.warnings.clone();
    warnings.extend(
        reviewer
            .harness
            .unsupported(&profile, "the reviewer's sandbox"),
    );
    Ok(warnings)
}

/// One reviewer round (spec §5.2): the reviewer runs in its own sandbox on its own clone of the
/// task branch (made from `repo.git`, so it sees exactly the collected commits and can change
/// nothing of the worker's), writes `review.md`, and sbxm saves it as `review-<round>.md` (and
/// `review.md`) under the reviewer's name. The sandbox and the clone are removed afterwards,
/// also when the round fails; a failed round leaves the task `reviewing/failed` and nothing used.
/// Scoped to every commit since the base when `Record::last_reviewed_commit` is unset (`full`),
/// else only to the commits since that review (issue 117, spec §5.2); a review that still finds
/// must-fix findings narrows the next one to this one's commit, and a clean one clears it so the
/// next review (the confirmatory full one, if this was narrow) starts fresh.
pub fn run_reviewer(env: &TaskEnv, prepared: &mut Prepared, round: u32) -> Result<Reviewed> {
    let id = prepared.record.id.clone();
    let reviewer = &env.config.reviewer;
    let clone = prepared.workspace.with_file_name(format!("{id}-review"));
    let sandbox = format!("sbxm-task-{id}-review-{}", reviewer.harness.as_str());
    let full = prepared.record.last_reviewed_commit.is_none();
    let tip = repo::branch_tip(&prepared.meta.join("repo.git"), &prepared.record.branch).ok();

    // Every earlier round's saved review, oldest first: the no-progress rule matches a claimed
    // repeat against any of them, not only the one right before (spec §5.3, decisions 174(b),
    // 177(o)), so a narrow round that misses an older finding can't hide it. A stale `review.md`
    // must never stand in for this round's. Walking the saved rounds (issue 169), rather than
    // every number below `round`, keeps the cost proportional to the files present: at
    // `round == u32::MAX`, trying every number first would be about 4.3 billion file reads.
    let earlier: Vec<String> = review::saved_rounds(&prepared.meta)
        .into_iter()
        .filter(|&r| r > 0 && r < round)
        .filter_map(|r| fs::read_to_string(prepared.meta.join(format!("review-{r}.md"))).ok())
        .collect();
    let _ = fs::remove_file(prepared.meta.join("review.md"));

    prepared
        .record
        .begin_review(now(), Process::current(env.probe))?;
    record::write(&prepared.meta, &prepared.record)?;

    let result = review_round(env, prepared, round, &clone, &sandbox, &earlier);

    // Always: the reviewer's sandbox and clone are gone, whatever happened.
    let _ = env.backend.remove(&sandbox);
    if let Err(e) = remove_with_retries(&clone) {
        prepared
            .record
            .notes
            .push(format!("could not remove {}: {e}", clone.display()));
    }
    match result {
        Ok(outcome) => {
            let must_fix = outcome.must_fix;
            prepared.record.last_reviewed_commit = if must_fix == 0 { None } else { tip };
            prepared.record.open_findings = Some(outcome.open);
            prepared.record.review = Some(record::ReviewResult {
                round,
                must_fix,
                full,
                repeat: outcome.repeat,
                risk: outcome.risk,
                risk_reasons: outcome.risk_reasons,
            });
            prepared.record.finish(Status::Completed)?;
            record::write(&prepared.meta, &prepared.record)?;
            Ok(Reviewed {
                round,
                must_fix,
                full,
                repeat: outcome.repeat,
                warnings: outcome.warnings,
                risk: outcome.risk,
            })
        }
        Err(e) => {
            prepared
                .record
                .notes
                .push(format!("review round {round}: {e:#}"));
            prepared.record.finish(Status::Failed)?;
            record::write(&prepared.meta, &prepared.record)?;
            Err(e)
        }
    }
}

/// What a reviewer round's own `review.md` decided, before any of it is recorded (decision 176).
struct ReviewOutcome {
    must_fix: u32,
    /// Whether a must-fix finding validly repeats one of `earlier`'s (spec §5.3, decisions
    /// 174(b), 177(o)).
    repeat: bool,
    /// One warning per invalid `Repeat of:` claim (decision 177(e)).
    warnings: Vec<String>,
    /// The must-fix findings open once this round is accepted ([`review::open_after`], decision
    /// 177(p)).
    open: Vec<OpenFinding>,
    /// The risk level (decision 176): the reviewer's own, raised to the path-rule floor but
    /// never lowered.
    risk: risk::Level,
    risk_reasons: Vec<String>,
}

/// One review round: its workspace, then the reviewer.
fn review_round(
    env: &TaskEnv,
    prepared: &mut Prepared,
    round: u32,
    clone: &Path,
    sandbox: &str,
    earlier: &[String],
) -> Result<ReviewOutcome> {
    open_review_workspace(env, prepared, round, clone, sandbox, earlier)?;
    run_review_agent(env, prepared, round, clone, sandbox, earlier)
}

/// The reviewer's context for a re-review (spec §5.3, decisions 174(b), 177(o)): every earlier
/// round's saved review, oldest first, so a finding from any of them (not only the one right
/// before) can still be named in a `Repeat of:` claim. `None` for round 1, which has nothing to
/// repeat.
fn combined_earlier_reviews(earlier: &[String]) -> Option<String> {
    if earlier.is_empty() {
        return None;
    }
    Some(
        earlier
            .iter()
            .enumerate()
            .map(|(i, text)| format!("## Review round {}\n\n{text}", i + 1))
            .collect::<Vec<_>>()
            .join("\n\n---\n\n"),
    )
}

/// Makes the reviewer's workspace: a clean clone of the task branch (a pull request's head for
/// a PR task) with the context and the prompt in `.sbxm-task/`, and the reviewer's own sandbox
/// over it. Nothing has run in the sandbox yet.
fn open_review_workspace(
    env: &TaskEnv,
    prepared: &mut Prepared,
    round: u32,
    clone: &Path,
    sandbox: &str,
    earlier: &[String],
) -> Result<()> {
    let reviewer = &env.config.reviewer;
    let repo_git = prepared.meta.join("repo.git");
    let branch = prepared.record.branch.clone();

    // A clone left by a killed run (and its sandbox) goes first.
    if clone.exists() {
        let _ = env.backend.remove(sandbox);
        remove_with_retries(clone)
            .with_context(|| format!("cannot clear {}, left by an earlier run", clone.display()))?;
    }
    let kit_set = kits::build_for(
        env.config_dir,
        &env.config.sandbox.profile,
        &[reviewer.harness],
        &Overrides {
            cpus: env.config.sandbox.cpus,
            memory: env.config.sandbox.memory.clone(),
        },
        &prepared.meta.join("kits-review"),
        env.backend,
    )?;
    let harness_kits = kit_set
        .get(reviewer.harness)
        .cloned()
        .context("no kits were built for the reviewer's harness")?;

    repo::clean_checkout(&repo_git, &branch, clone)?;
    let info = clone.join(".git").join("info");
    fs::create_dir_all(&info)?;
    fs::write(info.join("exclude"), format!("# sbxm\n{AGENT_DIR}/\n"))?;
    let (source_file, role) = match prepared.record.kind {
        Kind::Pr => ("issue.md", Role::ReviewerPr),
        Kind::Spec => ("source.md", Role::ReviewerSpec),
        Kind::Issue => ("issue.md", Role::Reviewer),
    };
    let issue = fs::read_to_string(prepared.meta.join(source_file)).unwrap_or_default();
    // Narrow to the commits since the last review when one is recorded, else the task's whole
    // scope (spec §5.2, issue 117): a PR task and the first review of an issue task never set
    // `last_reviewed_commit`, so both fall back to the full scope unchanged.
    let scope_base = prepared
        .record
        .last_reviewed_commit
        .clone()
        .unwrap_or_else(|| prepared.record.scope_base());
    let template = prompts::template(role, &env.config.prompts)?;
    let prompt = prompts::render(
        &template.name,
        &template.text,
        &[
            ("issue", &issue),
            ("number", &prepared.record.number.to_string()),
            ("branch", &branch),
            ("base", &prepared.record.base),
            ("scope_base", &scope_base),
            ("repo", &prepared.record.repo),
            ("gates_sandbox", &bullets(&env.config.gates.sandbox)),
            ("gates_host", &bullets(&env.config.gates.host)),
            ("review_path", ".sbxm-task/review.md"),
            ("previous_review_path", ".sbxm-task/previous-review.md"),
        ],
    )?;
    // The checkout is the worker's committed tree, so `.sbxm-task` may be a link it planted.
    let mut agent_files: Vec<(&str, &[u8])> = vec![
        (source_file, issue.as_bytes()),
        ("prompt.md", prompt.as_bytes()),
    ];
    let combined_earlier_reviews = combined_earlier_reviews(earlier);
    if let Some(text) = &combined_earlier_reviews {
        agent_files.push(("previous-review.md", text.as_bytes()));
    }
    repo::write_agent_files(clone, &agent_files, Existing::Replace)?;
    fs::write(
        prepared.meta.join(format!("reviewer-prompt-{round}.md")),
        &prompt,
    )?;

    // The record goes first: the create takes minutes, and a kill during it must leave the name
    // that `task rm` needs to find the sandbox.
    prepared.record.reviewer = Some(Agent {
        harness: reviewer.harness.as_str().to_owned(),
        model: reviewer.model.clone(),
        sandbox: sandbox.to_owned(),
        workspace: slashes(clone),
        // The reviewer's sandbox is rebuilt from the current config every round and is never
        // resumed, so it has no reconstruction settings to save (issue 118, PR 163 review M-2).
        profile: None,
        cpus: None,
        memory: None,
        run: None,
    });
    record::write(&prepared.meta, &prepared.record)?;
    env.backend
        .create(&CreateSpec {
            name: sandbox.to_owned(),
            agent: reviewer.harness.agent_arg().into(),
            workspace: clone.to_path_buf(),
            cpus: kit_set.resources.cpus,
            memory: kit_set.resources.memory.clone(),
            skills: harness_kits.skills_store,
            kits: harness_kits.dirs.clone(),
        })
        .with_context(|| format!("cannot create sandbox {sandbox}"))?;
    Ok(())
}

/// Runs the reviewer in its (already open) sandbox, saves its transcript, and reads and saves what
/// it wrote: `review-<round>.md` (and `review.md` when its first line has the count). The
/// must-fix claims, repeats and findings are [`ReviewOutcome`]'s, as is the risk level (decision
/// 176): the reviewer's own `Risk:` line (`Level::Unknown`, with a warning, when it is missing or
/// unparseable), raised to the path-rule floor computed from the task's own changed paths, but
/// never lowered.
fn run_review_agent(
    env: &TaskEnv,
    prepared: &mut Prepared,
    round: u32,
    clone: &Path,
    sandbox: &str,
    earlier: &[String],
) -> Result<ReviewOutcome> {
    let reviewer = &env.config.reviewer;
    // Before the reviewer runs: a git error here must stop the review, never drop the floor.
    let changed_paths = prepared
        .record
        .changed_paths(&prepared.meta.join("repo.git"))
        .context("cannot list the task's changed paths for the risk floor")?;
    let started = Instant::now();
    let result = headless::run(
        env.backend,
        sandbox,
        &in_sandbox_path(clone),
        reviewer.harness,
        AGENT_PROMPT,
        &HeadlessOpts {
            model: reviewer.model.clone(),
            // A review is worth the model's best effort (Codex defaults to a low one).
            high_effort: true,
            budget_usd: None,
            is_git_repo: true,
        },
        reviewer.time_limit,
    )
    .with_context(|| format!("cannot run the reviewer in {sandbox}"))?;

    let transcripts = prepared.meta.join("transcripts");
    fs::create_dir_all(&transcripts)?;
    fs::write(
        transcripts.join(format!("review-{round}.jsonl")),
        &result.transcript,
    )?;
    if let Some(agent) = prepared.record.reviewer.as_mut() {
        agent.run = Some(RunInfo {
            status: run_label(&result.status),
            usage: serde_json::json!({
                "input_tokens": result.usage.input_tokens,
                "output_tokens": result.usage.output_tokens,
                "cost_usd": result.usage.cost_usd,
            }),
            duration_s: started.elapsed().as_secs(),
        });
    }
    match &result.status {
        RunStatus::Completed => {}
        RunStatus::TimedOut => bail!(
            "the reviewer hit its time limit ({}s), so its review isn't used",
            reviewer.time_limit.as_secs()
        ),
        RunStatus::Failed(why) => bail!("the reviewer failed ({why}), so its review isn't used"),
    }

    let text = match repo::read_agent_file(clone, "review.md", REVIEW_CAP)? {
        AgentFile::Text(text) => text,
        AgentFile::Missing => bail!("the reviewer wrote no review.md"),
        AgentFile::TooLarge(_) => {
            bail!("the reviewer's review.md is larger than 1 MiB, so it isn't used")
        }
    };
    let saved = review::with_header(reviewer.harness.as_str(), reviewer.model.as_deref(), &text);
    fs::write(prepared.meta.join(format!("review-{round}.md")), &saved)?;
    let Some(must_fix) = review::must_fix_count(&text) else {
        bail!(
            "the reviewer's review.md doesn't start with 'Must-fix findings: <count>', so it isn't \
             used; read it in review-{round}.md"
        );
    };
    if !review::count_matches(must_fix, &text) {
        let listed = review::must_fix_findings(&text, round).len();
        bail!(
            "the reviewer's review.md says {must_fix} must-fix finding(s) but lists {listed} as \
             '### <id> - <title>' under '## Must fix', so it isn't used; read it in \
             review-{round}.md"
        );
    }
    fs::write(prepared.meta.join("review.md"), &saved)?;
    let earlier_refs: Vec<&str> = earlier.iter().map(String::as_str).collect();
    let repeat = review::repeats_a_must_fix_finding(&text, &earlier_refs);
    let warnings = review::invalid_repeat_claims(&text, &earlier_refs);
    let full = prepared.record.last_reviewed_commit.is_none();
    let earlier_open = match &prepared.record.open_findings {
        Some(open) => open.clone(),
        // A record from before the list: rebuilt from the saved reviews, never taken as empty.
        None => review::open_from_task_dir(&prepared.meta, round.saturating_sub(1)),
    };
    let open = review::open_after(Some(&earlier_open), &text, round, full);

    let (floor_level, floor_reasons) = risk::floor(&changed_paths, &env.config.risk);
    let (risk_level, risk_reasons) = match review::risk_level(&text) {
        Some(level) => {
            let mut reasons = review::risk_reasons(&text);
            let combined = level.max(floor_level);
            if combined > level {
                reasons.extend(floor_reasons);
            }
            (combined, reasons)
        }
        // Recorded as `Level::Unknown`, never silently `low` (decision 176(e)); `print_review`
        // and `run_pr` warn about it on their own, separately from a `Repeat of:` warning, so
        // this never changes `warnings`'s count for a review that has neither.
        None => (risk::Level::Unknown, review::risk_reasons(&text)),
    };
    Ok(ReviewOutcome {
        must_fix,
        repeat,
        warnings,
        open,
        risk: risk_level,
        risk_reasons,
    })
}
