//! The model half of `sbxm task ready-check --judge` (issue 726, decision 331(2)): the judgment
//! rules (open points in the prose, boundary cases under decision 244, the acceptance list against
//! the answers, a proposed `**Files:**` line, the size's fit) go to one model call in a fresh
//! sandbox that reaches only the model provider (decision 269(c): issue text is untrusted). Its
//! answer has a fixed format, parsed here. It advises: it never sets a label or gates the loop.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use super::finish::next_fence;
use super::ready::files_well_formed;
use crate::backend::{CreateSpec, SandboxBackend};
use crate::commands::new::require_secrets;
use crate::config::{GlobalConfig, Profile};
use crate::disk;
use crate::github::{GitHubBackend, Issue};
use crate::harness::Harness;
use crate::headless::{self, HeadlessOpts, RunStatus};
use crate::run::kits::{self, Overrides};
use crate::run::orchestrate::in_sandbox_path;

/// The embedded prompt (`prompts/ready-check.md`): the readiness rules and the answer's format.
pub const PROMPT: &str = include_str!("../../prompts/ready-check.md");

/// The answer's sections, in their order.
pub const SECTIONS: [&str; 5] = [
    "files",
    "open_points",
    "boundary_cases",
    "acceptance_vs_answers",
    "size",
];

/// The workspace files the prompt names: the issue, the default branch's paths, and the open PRs'
/// changed files.
pub const ISSUE_FILE: &str = "issue.md";
pub const PATHS_FILE: &str = "paths.txt";
pub const OPEN_PRS_FILE: &str = "open-prs.md";
/// The prompt's file in the workspace.
const PROMPT_FILE: &str = "prompt.md";

/// The profile the sandbox is made from (`profiles/ready-check/`): no hosts beyond the harness's
/// own provider.
pub const PROFILE: &str = "ready-check";

/// How long the model may take, enforced inside the sandbox (decision 114).
pub const TIME_LIMIT: Duration = Duration::from_secs(600);

/// The harness: Claude with its default model, as for workers.
const HARNESS: Harness = Harness::Claude;

/// A parsed answer: each section's text, trimmed, its lines kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The proposed paths, as a `**Files:**` line's text, or `none`.
    pub files: String,
    pub open_points: String,
    pub boundary_cases: String,
    pub acceptance_vs_answers: String,
    pub size: String,
}

impl Answer {
    /// The `**Files:**` line the answer proposes; `None` when it proposes none.
    pub fn files_line(&self) -> Option<String> {
        (!self.files.eq_ignore_ascii_case("none")).then(|| format!("**Files:** {}", self.files))
    }

    /// The sections as `name: text`, in their order.
    pub fn sections(&self) -> [(&'static str, &str); 5] {
        [
            (SECTIONS[0], &self.files),
            (SECTIONS[1], &self.open_points),
            (SECTIONS[2], &self.boundary_cases),
            (SECTIONS[3], &self.acceptance_vs_answers),
            (SECTIONS[4], &self.size),
        ]
    }
}

/// Reads the model's answer: every section once, none empty, `files:` one line of backticked
/// paths or `none`. Text before the first section and code fence lines are left out. `Err` says
/// why it doesn't parse.
pub fn parse(text: &str) -> Result<Answer, String> {
    let mut found: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim_start().starts_with("```") {
            continue;
        }
        let header = SECTIONS.iter().find_map(|name| {
            line.strip_prefix(name)
                .and_then(|rest| rest.strip_prefix(':'))
                .map(|rest| (*name, rest))
        });
        match header {
            Some((name, rest)) => {
                if found.iter().any(|(n, _)| *n == name) {
                    return Err(format!("the section `{name}:` is there twice"));
                }
                found.push((name, vec![rest.trim()]));
            }
            None => {
                if let Some((_, lines)) = found.last_mut() {
                    lines.push(line);
                }
            }
        }
    }
    let missing: Vec<String> = SECTIONS
        .iter()
        .filter(|name| !found.iter().any(|(n, _)| n == *name))
        .map(|name| format!("`{name}:`"))
        .collect();
    if !missing.is_empty() {
        return Err(format!("no {} section", missing.join(", ")));
    }
    let text_of = |name: &str| -> Result<String, String> {
        let lines = &found.iter().find(|(n, _)| *n == name).unwrap().1;
        let text = lines.join("\n").trim().to_owned();
        if text.is_empty() {
            return Err(format!(
                "the section `{name}:` is empty (it should say `none`)"
            ));
        }
        Ok(text)
    };
    let files = text_of("files")?;
    if !files.eq_ignore_ascii_case("none") && (files.contains('\n') || !files_well_formed(&files)) {
        return Err(format!(
            "`files:` `{files}` is neither `none` nor one line of backticked paths separated by \
             commas"
        ));
    }
    Ok(Answer {
        files,
        open_points: text_of("open_points")?,
        boundary_cases: text_of("boundary_cases")?,
        acceptance_vs_answers: text_of("acceptance_vs_answers")?,
        size: text_of("size")?,
    })
}

/// `body` with `line`, a `**Files:**` line, in place of its first `**Files:**` line outside code
/// fences, or else added before its first `**Depends on:**` line, with a blank line after it, or
/// else at its end. Line endings are kept.
pub fn with_files_line(body: &str, line: &str) -> String {
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    let mut fence = None;
    let mut unfenced = Vec::new();
    for (at, text) in lines.iter().enumerate() {
        let inside = fence.is_some();
        fence = next_fence(fence, text.trim_end_matches(['\r', '\n']));
        if !(inside || fence.is_some()) {
            unfenced.push(at);
        }
    }
    let starts = |at: &usize, label: &str| lines[*at].trim_start().starts_with(label);
    let ending = |text: &str| {
        let content = text.trim_end_matches(['\r', '\n']);
        text[content.len()..].to_owned()
    };
    if let Some(at) = unfenced.iter().find(|at| starts(at, "**Files:**")) {
        let mut out: Vec<String> = lines.iter().map(|l| (*l).to_owned()).collect();
        out[*at] = format!("{line}{}", ending(lines[*at]));
        return out.concat();
    }
    if let Some(at) = unfenced.iter().find(|at| starts(at, "**Depends on:**")) {
        let mut eol = ending(lines[*at]);
        if eol.is_empty() {
            eol = "\n".to_owned();
        }
        let mut out: Vec<String> = lines.iter().map(|l| (*l).to_owned()).collect();
        out.insert(*at, format!("{line}{eol}{eol}"));
        return out.concat();
    }
    let body = body.trim_end_matches(['\r', '\n']);
    format!("{body}\n\n{line}\n")
}

/// What the model reads, gathered on the host before any sandbox exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inputs {
    /// `issue.md`: the issue's number, title and body.
    pub issue: String,
    /// `paths.txt`: the default branch's paths, one per line.
    pub paths: String,
    /// `open-prs.md`: each open PR and the files it changes.
    pub open_prs: String,
}

/// Reads the inputs from GitHub: the default branch's paths and every open PR's changed files. A
/// read that fails is an error, so the model never judges on a part of them.
pub fn gather(repo: &str, github: &dyn GitHubBackend, issue: &Issue) -> Result<Inputs> {
    let branch = github
        .default_branch(repo)
        .with_context(|| format!("cannot read the default branch of {repo}"))?;
    let listed = github
        .repo_paths(repo, &branch)
        .with_context(|| format!("cannot list the paths of {repo} at {branch}"))?;
    let mut paths = String::new();
    for path in &listed {
        paths.push_str(path);
        paths.push('\n');
    }
    let numbers = github
        .prs_open(repo)
        .with_context(|| format!("cannot list the open PRs of {repo}"))?;
    let mut open_prs = String::new();
    if numbers.is_empty() {
        open_prs.push_str("No open pull requests.\n");
    }
    for number in numbers {
        let files = github
            .pr_files(repo, number)
            .with_context(|| format!("cannot read the files of PR #{number}"))?;
        open_prs.push_str(&format!("## PR #{number}\n\n"));
        for file in &files.files {
            open_prs.push_str(&format!("- `{}`\n", file.filename));
        }
        let unlisted = (files.changed_files as usize).saturating_sub(files.files.len());
        if unlisted > 0 {
            open_prs.push_str(&format!("- ({unlisted} more files not listed)\n"));
        }
        open_prs.push('\n');
    }
    Ok(Inputs {
        issue: format!("# #{} {}\n\n{}\n", issue.number, issue.title, issue.body),
        paths,
        open_prs,
    })
}

/// How the model run went: its final answer, wall time and cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub answer: String,
    pub seconds: u64,
    pub cost_usd: Option<f64>,
    /// The model the harness says it used, or its default.
    pub model: Option<String>,
}

/// Runs the model once on `inputs` for issue `number` in a fresh sandbox of [`PROFILE`] and
/// removes it, also when the run fails: checks the drive and builds the kits before it takes a
/// sandbox slot (decision 304), then writes the workspace, creates the sandbox and runs. An error
/// when the sandbox can't be created, the run fails or times out, or the sandbox can't be removed.
pub fn run(
    config_dir: &Path,
    guard: &disk::Guard,
    time_limit: Duration,
    number: u32,
    inputs: &Inputs,
    backend: &dyn SandboxBackend,
    warn: &mut dyn Write,
) -> Result<Reply> {
    let global = GlobalConfig::load(config_dir)?;
    disk::require(&global, config_dir, guard)?;
    if !global.base_dir.is_dir() {
        bail!(
            "base dir {} does not exist; create it or change base_dir in {}",
            global.base_dir.display(),
            config_dir.join("config.toml").display()
        );
    }
    // The profile and its secrets are checked before anything is written, so a missing one
    // leaves no trace (decisions 30, 58).
    let profile = Profile::load(global.profiles_dir(), PROFILE)?;
    if !profile.secrets.services.is_empty() {
        require_secrets(&profile.secrets.services, &backend.secret_services()?)?;
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let id = format!("issue-{number}-{stamp}-{}", std::process::id());
    let meta = global.base_dir.join(".sbxm").join(PROFILE).join(&id);
    let workspace = global.base_dir.join(PROFILE).join(&id);
    let sandbox = crate::sandbox_name::capped("sbxm-ready-", &id, "-claude");

    let mut made = Made::default();
    let result = (|| -> Result<Reply> {
        let kit_root = made.folder(&meta)?.join("kits");
        let kit_set = kits::build_for(
            config_dir,
            PROFILE,
            &[HARNESS],
            &Overrides::default(),
            None,
            &kit_root,
            backend,
        )?;
        let harness_kits = kit_set
            .get(HARNESS)
            .context("no kits were built for claude")?;
        let _slot = crate::slot::take(&global, &guard.slot, None, warn)?;
        made.folder(&workspace)?;
        for (name, text) in [
            (PROMPT_FILE, PROMPT),
            (ISSUE_FILE, inputs.issue.as_str()),
            (PATHS_FILE, inputs.paths.as_str()),
            (OPEN_PRS_FILE, inputs.open_prs.as_str()),
        ] {
            fs::write(workspace.join(name), text)
                .with_context(|| format!("cannot write {}", workspace.join(name).display()))?;
        }
        made.sandbox = true;
        backend
            .create(
                &CreateSpec {
                    name: sandbox.clone(),
                    agent: HARNESS.agent_arg().into(),
                    workspace: Some(workspace.clone()),
                    cpus: kit_set.resources.cpus,
                    memory: kit_set.resources.memory.clone(),
                    skills: harness_kits.skills_store,
                    kits: harness_kits.dirs.clone(),
                    template: None,
                    deny_network: harness_kits.deny_network.clone().unwrap_or_default(),
                    ttl: None,
                },
                None,
            )
            .with_context(|| format!("cannot create the ready-check sandbox {sandbox}"))?;
        let started = Instant::now();
        let reply = headless::run(
            backend,
            &sandbox,
            &in_sandbox_path(&workspace),
            HARNESS,
            &format!(
                "Read the file {PROMPT_FILE} in the current directory and follow it exactly. \
                 Your final reply must be only the five sections it asks for."
            ),
            &HeadlessOpts {
                model: None,
                high_effort: false,
                budget_usd: None,
                is_git_repo: false,
                resume: None,
            },
            time_limit,
        )
        .with_context(|| format!("cannot run the model in {sandbox}"))?;
        match reply.status {
            RunStatus::Completed => {}
            RunStatus::TimedOut => {
                bail!("the model timed out after {}s", time_limit.as_secs())
            }
            RunStatus::Failed(why) => bail!("the model run failed: {why}"),
        }
        Ok(Reply {
            answer: reply.answer,
            seconds: started.elapsed().as_secs(),
            cost_usd: reply.usage.cost_usd,
            model: reply.model.or_else(|| HARNESS.model_or_default(None)),
        })
    })();

    // Always, once create was asked for, whatever became of the run; the folders only once the
    // sandbox that mounts one is gone.
    let removed = if made.sandbox {
        backend.remove(&sandbox, None).with_context(|| {
            format!("cannot remove the ready-check sandbox {sandbox}; run `sbx rm -f {sandbox}`")
        })
    } else {
        Ok(())
    };
    if removed.is_ok() {
        for folder in made.folders.iter().rev() {
            if let Err(e) = fs::remove_dir_all(folder) {
                writeln!(warn, "warning: cannot remove {}: {e}", folder.display())?;
            }
        }
    }
    match (result, removed) {
        (Ok(reply), Ok(())) => Ok(reply),
        (Err(e), Ok(())) => Err(e),
        (Ok(_), Err(gone)) => Err(gone),
        (Err(e), Err(gone)) => Err(e.context(format!("{gone:#}"))),
    }
}

/// What a run made, so it removes only that: the folders it created (never one that was there
/// before) and whether it asked for the sandbox.
#[derive(Default)]
struct Made {
    folders: Vec<PathBuf>,
    sandbox: bool,
}

impl Made {
    /// Creates `folder`, which must not exist yet, with its missing parents, and records it.
    fn folder(&mut self, folder: &Path) -> Result<PathBuf> {
        if let Some(parent) = folder.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        fs::create_dir(folder).with_context(|| format!("cannot create {}", folder.display()))?;
        self.folders.push(folder.to_owned());
        Ok(folder.to_owned())
    }
}
