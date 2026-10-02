//! `sbxm task` orchestration, one function per phase (spec §5, §1). Every refusal happens before
//! the first write or backend call; once folders are reserved, a failure removes them again.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use std::time::Instant;

use super::config::TaskConfig;
use super::gates::{self, GateOutcome, HostRunner};
use super::prompts::{self, Role};
use super::record::{
    self, Agent, GateResult, Kind, NewTask, Process, ProcessProbe, Record, RunInfo, Stage, Status,
};
use super::repo::{self, AgentFile, BUNDLE_CAP, Identity};
use super::review;
use super::select::{self, Selection};
use crate::backend::{CreateSpec, ExecSpec, SandboxBackend, Stdin};
use crate::config::{GlobalConfig, Profile};
use crate::github::{GitHubBackend, IssueText, PrInfo, PrState};
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
/// finished, or again after earlier gates ended. The reason for a refusal says what to do.
pub fn check_can_gate(task: &Record, probe: &dyn ProcessProbe) -> Result<()> {
    let id = &task.id;
    let number = task.number;
    match (task.stage, task.status) {
        (Stage::Working | Stage::Fixing, Status::Completed | Status::TimedOut)
        | (Stage::Gating, Status::Passed | Status::GatesFailed) => Ok(()),
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
        // Gates whose sbxm process is gone never finished: `run_gates` gives them up and re-runs.
        (Stage::Gating, Status::Running) if task.is_interrupted(probe) => Ok(()),
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
            },
            now(),
            Process::current(ctx.probe),
        );
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

/// Chooses the issues to start (spec §8). With nothing to pick, the error says why for each.
pub fn select_issues(ctx: &Ctx, explicit: Option<&[u32]>, workers: usize) -> Result<Selection> {
    let base = GlobalConfig::load(ctx.config_dir)?.base_dir;
    let open = ctx.github.issues_open(ctx.repo)?;
    let taken: Vec<u32> = record::load_all(&base)?
        .into_iter()
        .filter(|r| r.kind == Kind::Issue)
        .map(|r| r.number)
        .collect();
    let selection = select::select(&open, &taken, explicit, workers);
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
    let branch = id.clone();
    let prompt = prompts::render(
        &template.name,
        &template.text,
        &[
            ("issue", &issue.text),
            ("number", &issue.number.to_string()),
            ("branch", &branch),
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
        repo::create_branch(&repo_git, &branch, ctx.base_branch)?;
        repo::clone_workspace(&repo_git, &workspace, &branch, ctx.identity)?;

        // The agent's `git add -A` must not pick up sbxm's own files.
        let info = workspace.join(".git").join("info");
        fs::create_dir_all(&info)?;
        fs::write(info.join("exclude"), format!("# sbxm\n{AGENT_DIR}/\n"))?;
        let agent_dir = workspace.join(AGENT_DIR);
        fs::create_dir_all(&agent_dir)?;
        fs::write(agent_dir.join("issue.md"), &issue.text)?;
        fs::write(agent_dir.join("prompt.md"), &prompt)?;
        fs::write(meta.join("issue.md"), &issue.text)?;
        fs::write(meta.join("worker-prompt.md"), &prompt)?;

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

        let mut task = Record::new(
            &NewTask {
                kind: Kind::Issue,
                number: issue.number,
                repo: ctx.repo,
                title: &issue.title,
                base: ctx.base_branch,
                branch: &branch,
                config_hash: &harness_kits.config_hash,
            },
            now(),
            Process::current(ctx.probe),
        );
        task.worker = Some(Agent {
            harness: worker.harness.as_str().to_owned(),
            model: worker.model.clone(),
            sandbox: sandbox.clone(),
            workspace: slashes(&workspace),
            run: None,
        });
        record::write(&meta, &task)?;
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
    if prepared.record.abandon_interrupted_gates(env.probe) {
        prepared
            .record
            .notes
            .push("earlier gates were interrupted before they finished".to_owned());
    }
    prepared
        .record
        .begin_gating(now(), Process::current(env.probe))?;
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
        let id = prepared.record.id.clone();
        let checkout = prepared.workspace.with_file_name(format!("{id}-gates"));
        // A folder left by a killed run (or one a build still held) must not fail this run.
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
                outcomes.extend(gates::run_host_tier(
                    env.host,
                    &checkout,
                    phase,
                    &gates.host,
                    gates.timeout,
                ));
                if let Err(e) = remove_with_retries(&checkout) {
                    prepared
                        .record
                        .notes
                        .push(format!("could not remove {}: {e}", checkout.display()));
                }
            }
            Err(e) => outcomes.push(GateOutcome {
                result: GateResult {
                    phase: phase.to_owned(),
                    tier: "host".to_owned(),
                    command: "(clean checkout)".to_owned(),
                    exit: None,
                    passed: false,
                },
                output_tail: format!("{e:#}"),
                timed_out: false,
            }),
        }
    }

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
    let worker = &ctx.config.worker;
    let sandbox = prepared
        .record
        .worker
        .as_ref()
        .context("the task has no worker")?
        .sandbox
        .clone();
    prepared
        .record
        .advance(Stage::Working, now(), Process::current(ctx.probe))?;
    record::write(&prepared.meta, &prepared.record)?;

    let started = Instant::now();
    let opts = HeadlessOpts {
        model: worker.model.clone(),
        high_effort: false,
        budget_usd: None,
        is_git_repo: true,
    };
    let result = match headless::run(
        ctx.backend,
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
    let commits = collect(ctx.backend, prepared, &sandbox, &mut status, &mut notes)?;
    prepared.record.notes.extend(notes.iter().cloned());
    record::write(&prepared.meta, &prepared.record)?;
    Ok(Worked {
        status,
        notes,
        commits,
    })
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
    let (branch, base) = (prepared.record.branch.clone(), prepared.record.base.clone());
    let ws = in_sandbox_path(&prepared.workspace)
        .to_string_lossy()
        .into_owned();
    let in_sandbox = |argv: &[&str]| ExecSpec {
        workdir: None,
        argv: argv.iter().map(|a| (*a).to_owned()).collect(),
        stdin: Stdin::Closed,
    };
    let mut failed = |why: String, status: &mut RunStatus, record: &mut Record| {
        notes.push(format!("could not collect the worker's commits: {why}"));
        *status = RunStatus::Failed(why);
        record.status = Status::Failed;
    };

    // The fixed bundle command runs in the sandbox; nothing runs git in the agent's clone here.
    let origin = format!("^origin/{base}");
    let mut commits = 0;
    match backend.exec(
        sandbox,
        &in_sandbox(&[
            "git",
            "-C",
            &ws,
            "bundle",
            "create",
            ".sbxm-task/branch.bundle",
            &branch,
            &origin,
        ]),
    ) {
        Ok(out) if out.exit_code == Some(0) => {
            let repo_git = prepared.meta.join("repo.git");
            match repo::fetch_bundle(&repo_git, &prepared.workspace, &branch, BUNDLE_CAP)
                .and_then(|()| repo::commits_ahead(&repo_git, &base, &branch))
            {
                Ok(n) => commits = n,
                Err(e) => failed(format!("{e:#}"), status, &mut prepared.record),
            }
        }
        Ok(out) if out.stderr.contains("empty bundle") => {
            notes.push("the worker made no commits".to_owned());
        }
        Ok(out) => failed(
            format!(
                "`git bundle create` failed in the sandbox: {}",
                out.stderr.trim()
            ),
            status,
            &mut prepared.record,
        ),
        Err(e) => failed(format!("{e:#}"), status, &mut prepared.record),
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

/// Whether a review may start now (`task review`). The reason for a refusal says what to do.
pub fn check_can_review(task: &Record, probe: &dyn ProcessProbe) -> Result<()> {
    let id = &task.id;
    let number = task.number;
    match (task.stage, task.status) {
        (Stage::Working | Stage::Fixing, Status::Completed | Status::TimedOut)
        | (Stage::Gating, Status::Passed | Status::GatesFailed)
        | (Stage::Reviewing, Status::Failed) => Ok(()),
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
        (Stage::Working | Stage::Fixing, _) => bail!(
            "the worker failed for task {id}, so there is nothing to review; see `sbxm task status --issue {number}`"
        ),
        // Gates whose process is gone are given up and run again by the review itself.
        (Stage::Gating, Status::Running) if task.is_interrupted(probe) => Ok(()),
        (Stage::Gating, _) => bail!(
            "gates are running for task {id}, or were interrupted; see `sbxm task status --issue {number}`"
        ),
        (Stage::Reviewing, _) => bail!(
            "task {id} is already being reviewed or has been (stage reviewing); see `sbxm task status --issue {number}`"
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
    /// One entry per reviewer round that finished (one or two).
    pub rounds: Vec<Reviewed>,
    /// Whether the single fix round ran.
    pub fix_ran: bool,
    /// Set when gates failed before a review or after the fix round: the review stops there.
    pub gates_failed: Option<GateResult>,
    /// Must-fix findings in the last review (reported, not an error).
    pub must_fix_left: u32,
    pub warnings: Vec<String>,
}

/// Spec §5.2, issue tasks: checks first (nothing is created if a secret is missing), gates when
/// they haven't passed since the last change, a reviewer, and if it found must-fix problems and
/// no fix round was used yet: one fix round by the worker, its commits collected, gates again and
/// one more review. The task ends `ready` (findings left are reported, not an error), or stops
/// where gates failed or a step failed.
pub fn review_issue(env: &TaskEnv, prepared: &mut Prepared) -> Result<ReviewReport> {
    check_can_review(&prepared.record, env.probe)?;
    let mut report = ReviewReport {
        warnings: check_reviewer(env, prepared)?,
        ..ReviewReport::default()
    };

    let gates_current = matches!(
        (prepared.record.stage, prepared.record.status),
        (Stage::Gating, Status::Passed) | (Stage::Reviewing, Status::Failed)
    );
    if !gates_current {
        let gated = run_gates(env, prepared, "before-review", Tiers::ALL)?;
        if let Some(failed) = gated.failed {
            report.gates_failed = Some(failed);
            return Ok(report);
        }
    }

    let mut round = if prepared.record.fix_round { 2 } else { 1 };
    loop {
        let reviewed = run_reviewer(env, prepared, round)?;
        report.must_fix_left = reviewed.must_fix;
        let must_fix = reviewed.must_fix;
        report.rounds.push(reviewed);
        if must_fix == 0 || prepared.record.fix_round {
            break;
        }
        run_fix_round(env, prepared)?;
        report.fix_ran = true;
        let gated = run_gates(env, prepared, "after-fix", Tiers::ALL)?;
        if let Some(failed) = gated.failed {
            report.gates_failed = Some(failed);
            return Ok(report);
        }
        round = 2;
    }

    prepared
        .record
        .advance(Stage::Ready, now(), Process::current(env.probe))?;
    record::write(&prepared.meta, &prepared.record)?;
    Ok(report)
}

/// The one fix round (spec §5.2): the worker, in its own sandbox, gets the review and a fix
/// prompt; its new commits are collected like the first time. A failed run, or commits that can't
/// be collected, stop the review (the task is `fixing/failed`).
fn run_fix_round(env: &TaskEnv, prepared: &mut Prepared) -> Result<()> {
    let worker = &env.config.worker;
    let sandbox = prepared
        .record
        .worker
        .as_ref()
        .context("the task has no worker")?
        .sandbox
        .clone();
    prepared
        .record
        .advance(Stage::Fixing, now(), Process::current(env.probe))?;
    prepared.record.fix_round = true;
    record::write(&prepared.meta, &prepared.record)?;

    let review = fs::read_to_string(prepared.meta.join("review.md"))
        .context("the review to fix is missing; run the review again")?;
    let template = prompts::template(Role::Fix, &env.config.prompts)?;
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
        ],
    )?;
    let agent_dir = prepared.workspace.join(AGENT_DIR);
    fs::create_dir_all(&agent_dir)?;
    fs::write(agent_dir.join("review.md"), &review)?;
    fs::write(agent_dir.join("fix-prompt.md"), &prompt)?;
    fs::write(prepared.meta.join("fix-prompt.md"), &prompt)?;

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
}

/// Checks before a review creates anything (spec §5.2): the reviewer's provider secret and the
/// profile's secrets are stored, and the profile loads. Returns the warnings to print.
pub fn check_reviewer(env: &TaskEnv, _task: &Prepared) -> Result<Vec<String>> {
    let global = GlobalConfig::load(env.config_dir)?;
    let reviewer = &env.config.reviewer;
    let profile = Profile::load(global.profiles_dir(), &env.config.sandbox.profile)?;
    check_secrets(
        env.backend,
        reviewer.harness,
        "the reviewer",
        &env.config.sandbox.profile,
        &profile,
    )?;
    let mut warnings = env.config.warnings.clone();
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
pub fn run_reviewer(env: &TaskEnv, prepared: &mut Prepared, round: u32) -> Result<Reviewed> {
    let id = prepared.record.id.clone();
    let reviewer = &env.config.reviewer;
    let clone = prepared.workspace.with_file_name(format!("{id}-review"));
    let sandbox = format!("sbxm-task-{id}-review-{}", reviewer.harness.as_str());

    // Round 2 reads round 1's review; a stale `review.md` must never stand in for this round's.
    let previous = (round > 1)
        .then(|| fs::read_to_string(prepared.meta.join(format!("review-{}.md", round - 1))).ok())
        .flatten();
    let _ = fs::remove_file(prepared.meta.join("review.md"));

    prepared
        .record
        .begin_review(now(), Process::current(env.probe))?;
    record::write(&prepared.meta, &prepared.record)?;

    let result = review_round(env, prepared, round, &clone, &sandbox, previous.as_deref());

    // Always: the reviewer's sandbox and clone are gone, whatever happened.
    let _ = env.backend.remove(&sandbox);
    if let Err(e) = remove_with_retries(&clone) {
        prepared
            .record
            .notes
            .push(format!("could not remove {}: {e}", clone.display()));
    }
    match result {
        Ok(reviewed) => {
            prepared.record.finish(Status::Completed)?;
            record::write(&prepared.meta, &prepared.record)?;
            Ok(reviewed)
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

fn review_round(
    env: &TaskEnv,
    prepared: &mut Prepared,
    round: u32,
    clone: &Path,
    sandbox: &str,
    previous: Option<&str>,
) -> Result<Reviewed> {
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
    let agent_dir = clone.join(AGENT_DIR);
    fs::create_dir_all(&agent_dir)?;
    let issue = fs::read_to_string(prepared.meta.join("issue.md")).unwrap_or_default();
    fs::write(agent_dir.join("issue.md"), &issue)?;
    if let Some(text) = previous {
        fs::write(agent_dir.join("previous-review.md"), text)?;
    }
    let template = prompts::template(Role::Reviewer, &env.config.prompts)?;
    let prompt = prompts::render(
        &template.name,
        &template.text,
        &[
            ("issue", &issue),
            ("number", &prepared.record.number.to_string()),
            ("branch", &branch),
            ("base", &prepared.record.base),
            ("repo", &prepared.record.repo),
            ("gates_sandbox", &bullets(&env.config.gates.sandbox)),
            ("gates_host", &bullets(&env.config.gates.host)),
            ("review_path", ".sbxm-task/review.md"),
            ("previous_review_path", ".sbxm-task/previous-review.md"),
        ],
    )?;
    fs::write(agent_dir.join("prompt.md"), &prompt)?;
    fs::write(
        prepared.meta.join(format!("reviewer-prompt-{round}.md")),
        &prompt,
    )?;

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
    prepared.record.reviewer = Some(Agent {
        harness: reviewer.harness.as_str().to_owned(),
        model: reviewer.model.clone(),
        sandbox: sandbox.to_owned(),
        workspace: slashes(clone),
        run: None,
    });
    record::write(&prepared.meta, &prepared.record)?;

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
    fs::write(prepared.meta.join("review.md"), &saved)?;
    Ok(Reviewed { round, must_fix })
}
