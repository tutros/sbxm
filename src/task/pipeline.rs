//! `sbxm task` orchestration, one function per phase (spec §5, §1). Every refusal happens before
//! the first write or backend call; once folders are reserved, a failure removes them again.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::config::TaskConfig;
use super::prompts::{self, Role};
use super::record::{self, Agent, Kind, NewTask, Process, ProcessProbe, Record};
use super::repo::{self, Identity};
use super::select::{self, Selection};
use crate::backend::{CreateSpec, SandboxBackend};
use crate::config::{GlobalConfig, Profile};
use crate::github::{GitHubBackend, IssueText};
use crate::run::kits::{self, HarnessKits, Overrides};
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
