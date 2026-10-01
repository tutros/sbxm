//! Fixtures for the `sbxm task` pipeline tests: a target repo config, a local bare repo
//! standing in for GitHub, and a way to play the agent through the fake backend.

use std::fs;
use std::path::{Path, PathBuf};

use super::{Env, git, git_origin};
use sbxm::backend::{ExecOutput, ExecSpec, FakeBackend};
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{Issue, IssueText};
use sbxm::task::config::TaskConfig;
use sbxm::task::gates::{FakeHostRunner, HostRunner};
use sbxm::task::pipeline::Ctx;
use sbxm::task::record::ProcessProbe;
use sbxm::task::repo::Identity;

/// Claude's stream-json for a run that finished (a real capture).
pub const CLAUDE_DONE: &str = include_str!("../../src/fixtures/claude-stream-json-pong.jsonl");

pub struct Probe;

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        Some(0)
    }
}

pub struct Fixture {
    pub env: Env,
    pub origin: PathBuf,
    pub config: TaskConfig,
    pub identity: Identity,
}

pub fn fixture() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\", \"cargo fmt --check\"]\n",
    )
}

pub fn fixture_with(task_toml: &str) -> Fixture {
    let env = Env::new();
    let origin = git_origin(env.tmp.path());
    let repo_root = env.tmp.path().join("target-repo");
    fs::create_dir_all(&repo_root).unwrap();
    fs::write(repo_root.join("sbxm-task.toml"), task_toml).unwrap();
    let config = TaskConfig::load(&repo_root).unwrap();
    Fixture {
        env,
        origin,
        config,
        identity: Identity {
            name: "Dev".into(),
            email: "dev@example.com".into(),
        },
    }
}

pub fn issue_text(number: u32) -> IssueText {
    IssueText {
        number,
        title: format!("Fix {number}"),
        state: "OPEN".into(),
        text: format!("title:\tFix {number}\nstate:\tOPEN\n--\n**Acceptance criteria:** do it\n"),
    }
}

pub fn open_issue(number: u32, labels: &[&str], body: &str) -> Issue {
    Issue {
        number,
        title: format!("Fix {number}"),
        labels: labels.iter().map(|l| (*l).to_owned()).collect(),
        body: body.to_owned(),
    }
}

pub fn ctx<'a>(
    f: &'a Fixture,
    source: &'a str,
    backend: &'a FakeBackend,
    github: &'a FakeGitHub,
) -> Ctx<'a> {
    // Host gates off in most tests; a runner that records everything and runs nothing.
    let host: &'static FakeHostRunner = Box::leak(Box::default());
    ctx_with_host(f, source, backend, github, host)
}

pub fn ctx_with_host<'a>(
    f: &'a Fixture,
    source: &'a str,
    backend: &'a FakeBackend,
    github: &'a FakeGitHub,
    host: &'a dyn HostRunner,
) -> Ctx<'a> {
    Ctx {
        host,
        config_dir: Box::leak(Box::new(f.env.config_dir())),
        repo: "o/r",
        clone_source: source,
        base_branch: "main",
        config: &f.config,
        backend,
        github,
        identity: &f.identity,
        probe: &Probe,
    }
}

/// Prepares issue 41 and runs its worker (against `backend`), leaving it at `working/completed`.
pub fn worked_task(f: &Fixture, backend: &FakeBackend) -> sbxm::task::pipeline::Prepared {
    let github = FakeGitHub::default();
    let source = source(f);
    let ctx = ctx(f, &source, backend, &github);
    let mut prepared = sbxm::task::pipeline::prepare(&ctx, &issue_text(41)).unwrap();
    sbxm::task::pipeline::run_worker(&ctx, &mut prepared).unwrap();
    prepared
}

pub fn source(f: &Fixture) -> String {
    f.origin.to_str().unwrap().to_owned()
}

pub fn backend() -> FakeBackend {
    FakeBackend::with_secrets(&["anthropic"])
}

pub fn ok(stdout: &str) -> ExecOutput {
    ExecOutput {
        stdout: stdout.to_owned(),
        stderr: String::new(),
        exit_code: Some(0),
    }
}

/// What the agent does inside its sandbox when the fake backend is asked to run its command.
#[derive(Clone, Default)]
pub struct Play {
    /// Files committed one by one on the task branch.
    pub commits: Vec<String>,
    /// Written to `.sbxm-task/result.md` (bytes, so a test can make it huge).
    pub result_md: Option<Vec<u8>>,
    /// Replaces the bundle the sandbox's `git bundle create` would have written.
    pub bundle_bytes: Option<Vec<u8>>,
}

pub fn is_headless(spec: &ExecSpec) -> bool {
    spec.argv.iter().any(|a| a == "claude")
}

pub fn is_bundle(spec: &ExecSpec) -> bool {
    spec.argv.iter().any(|a| a == "bundle")
}

/// Like [`play`], for several tasks at once: the sandbox name (`sbxm-task-issue-<n>-<harness>`)
/// says which workspace under `<base_dir>/tasks/` the agent is working in.
pub fn play_tasks(
    base_dir: &Path,
    base: &str,
    what: Play,
) -> impl Fn(&str, &ExecSpec) + Send + Sync + use<> {
    let base_dir = base_dir.to_path_buf();
    let base = base.to_owned();
    move |sandbox: &str, spec: &ExecSpec| {
        let Some(number) = sandbox
            .strip_prefix("sbxm-task-issue-")
            .and_then(|rest| rest.split('-').next())
        else {
            return;
        };
        let branch = format!("issue-{number}");
        let workspace = base_dir.join("tasks").join(&branch);
        play(&workspace, &base, &branch, what.clone())(sandbox, spec);
    }
}

/// A hook for `FakeBackend::with_exec_hook` that plays the agent in `workspace`: on the headless
/// command it commits files and writes `result.md`; on the fixed bundle command it runs the real
/// `git bundle create` there (as the sandbox's git would).
pub fn play(
    workspace: &Path,
    base: &str,
    branch: &str,
    what: Play,
) -> impl Fn(&str, &ExecSpec) + Send + Sync + use<> {
    let workspace = workspace.to_path_buf();
    let (base, branch) = (base.to_owned(), branch.to_owned());
    move |_sandbox: &str, spec: &ExecSpec| {
        if is_headless(spec) {
            for file in &what.commits {
                fs::write(workspace.join(file), "x\n").unwrap();
                git(&workspace, &["add", "-A"]);
                git(&workspace, &["commit", "-q", "-m", file]);
            }
            if let Some(bytes) = &what.result_md {
                let dir = workspace.join(".sbxm-task");
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join("result.md"), bytes).unwrap();
            }
        } else if is_bundle(spec) {
            let dir = workspace.join(".sbxm-task");
            fs::create_dir_all(&dir).unwrap();
            if let Some(bytes) = &what.bundle_bytes {
                fs::write(dir.join("branch.bundle"), bytes).unwrap();
            } else if !what.commits.is_empty() {
                git(
                    &workspace,
                    &[
                        "bundle",
                        "create",
                        ".sbxm-task/branch.bundle",
                        &branch,
                        &format!("^origin/{base}"),
                    ],
                );
            }
        }
    }
}
