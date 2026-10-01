//! `sbxm task` orchestration, one function per phase (spec §5, §1). Every refusal happens before
//! the first write or backend call; once folders are reserved, a failure removes them again.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use std::time::Instant;

use super::config::TaskConfig;
use super::prompts::{self, Role};
use super::record::{
    self, Agent, Kind, NewTask, Process, ProcessProbe, Record, RunInfo, Stage, Status,
};
use super::repo::{self, AgentFile, BUNDLE_CAP, Identity};
use super::select::{self, Selection};
use crate::backend::{CreateSpec, ExecSpec, SandboxBackend, Stdin};
use crate::config::{GlobalConfig, Profile};
use crate::github::{GitHubBackend, IssueText};
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
}

/// A task that is ready for its worker.
#[derive(Debug)]
pub struct Prepared {
    pub record: Record,
    /// `<base>/.sbxm/tasks/<id>/`
    pub meta: PathBuf,
    /// `<base>/tasks/<id>/`: the worker's clone, mounted in its sandbox.
    pub workspace: PathBuf,
    pub kits: HarnessKits,
    /// One line each, to print before the run.
    pub warnings: Vec<String>,
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
    let stored = ctx.backend.secret_services()?;
    let mut needed = vec![(
        worker.harness.provider_secret().to_owned(),
        format!("the worker, {}", worker.harness.as_str()),
    )];
    needed.extend(profile.secrets.services.iter().map(|s| {
        (
            s.clone(),
            format!(
                "secrets.services of profile '{}'",
                ctx.config.sandbox.profile
            ),
        )
    }));
    if let Some((missing, reason)) = needed.iter().find(|(s, _)| !stored.contains(s)) {
        bail!(
            "secret '{missing}' (needed by {reason}) is not stored in sbx; add it with \
             `sbx secret set {missing}` or import it with `sbx setup`"
        );
    }
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
            kits,
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
                })
            })
            .collect()
    })
}

fn start_one(ctx: &Ctx, number: u32) -> TaskReport {
    let mut warnings = Vec::new();
    let result = (|| {
        let issue = ctx.github.issue(ctx.repo, number)?;
        let mut prepared = prepare(ctx, &issue)?;
        warnings.clone_from(&prepared.warnings);
        run_worker(ctx, &mut prepared)
    })();
    TaskReport {
        number,
        warnings,
        result,
    }
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
    let commits = collect(ctx, prepared, &sandbox, &mut status, &mut notes)?;
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
    ctx: &Ctx,
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
    match ctx.backend.exec(
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
    if let Ok(out) = ctx.backend.exec(
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
