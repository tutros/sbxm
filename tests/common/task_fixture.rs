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

/// Codex's ndjson for a run that finished (a real capture).
pub const CODEX_DONE: &str = include_str!("../../src/fixtures/codex-ndjson-pong.jsonl");

/// What the reviewer does in its clone when the fake backend is asked to run its headless command:
/// writes `.sbxm-task/review.md` (nothing when `review` is `None`), and notes what it could see.
pub fn play_reviewer(
    base_dir: &Path,
    task: &str,
    review: Option<String>,
) -> impl Fn(&str, &ExecSpec) + Send + Sync + use<> {
    let clone = base_dir.join("tasks").join(format!("{task}-review"));
    move |_sandbox: &str, spec: &ExecSpec| {
        if spec.argv.iter().any(|a| a == "codex")
            && let Some(text) = &review
        {
            let dir = clone.join(".sbxm-task");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("review.md"), text).unwrap();
        }
    }
}

/// Like [`play_reviewer`], but the n-th reviewer run writes `reviews[n]` (the last repeats).
pub fn play_reviews(
    base_dir: &Path,
    task: &str,
    reviews: Vec<String>,
) -> impl Fn(&str, &ExecSpec) + Send + Sync + use<> {
    let clone = base_dir.join("tasks").join(format!("{task}-review"));
    let runs = std::sync::atomic::AtomicUsize::new(0);
    move |sandbox: &str, spec: &ExecSpec| {
        // Any harness can be the reviewer; it is the sandbox that says it is one.
        let headless = spec.argv.iter().any(|a| a == "codex" || a == "claude");
        if sandbox.contains("-review-") && headless {
            let n = runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let text = &reviews[n.min(reviews.len() - 1)];
            let dir = clone.join(".sbxm-task");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("review.md"), text).unwrap();
        }
    }
}

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
        open: true,
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

/// What GitHub keeps for a pull request: a commit on top of `main`, at `refs/pull/<n>/head` of the
/// stand-in origin. Returns the commit's id.
pub fn add_pr_head(f: &Fixture, number: u32) -> String {
    let scratch = f.env.tmp.path().join(format!("pr-scratch-{number}"));
    git(
        f.env.tmp.path(),
        &[
            "clone",
            "-q",
            f.origin.to_str().unwrap(),
            scratch.to_str().unwrap(),
        ],
    );
    fs::write(scratch.join("pr.txt"), format!("change from PR {number}\n")).unwrap();
    git(&scratch, &["add", "-A"]);
    git(&scratch, &["commit", "-q", "-m", "the PR's change"]);
    let sha = git(&scratch, &["rev-parse", "HEAD"]);
    git(
        &scratch,
        &[
            "push",
            "-q",
            f.origin.to_str().unwrap(),
            &format!("HEAD:refs/pull/{number}/head"),
        ],
    );
    sha
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
    spec.argv.iter().any(|a| a == "claude" || a == "codex")
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
    move |sandbox: &str, spec: &ExecSpec| {
        // A reviewer's sandbox is not the worker's, whatever harness it runs.
        if sandbox.contains("-review-") {
            return;
        }
        if is_headless(spec) {
            // Each headless run (the worker's, then a fix round's) writes different content, so
            // each makes a real commit.
            static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let run = RUNS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            for file in &what.commits {
                let path = workspace.join(file);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, format!("x{run}\n")).unwrap();
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

/// A clean review's text.
pub const CLEAN_REVIEW: &str = "Must-fix findings: 0\n\nNothing found.\n";

/// Writes `text` to `idea.md` in the fixture's temp dir: a spec task's source file.
pub fn spec_file(f: &Fixture, text: &str) -> PathBuf {
    let path = f.env.tmp.path().join("idea.md");
    fs::write(&path, text).unwrap();
    path
}

/// Starts a spec task from `idea.md`, runs its worker (one commit) and a clean review: it ends
/// `ready`. Returns the spec file and the task id.
pub fn ready_spec_task(f: &Fixture) -> (PathBuf, String) {
    use sbxm::task::{pipeline, record};
    let spec = spec_file(f, "Build a thing.\n");
    let (id, _) = record::spec_id(&spec).unwrap();
    let worker = play(
        &f.env.base_dir().join("tasks").join(&id),
        "main",
        &id,
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(&f.env.base_dir(), &id, vec![CLEAN_REVIEW.to_owned()]);
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, exec| {
            worker(sandbox, exec);
            reviewer(sandbox, exec);
        });
    let github = FakeGitHub::default();
    let source = source(f);
    let context = ctx(f, &source, &backend, &github);
    let mut prepared = pipeline::prepare_spec(&context, &spec).unwrap();
    pipeline::run_worker(&context, &mut prepared).unwrap();
    pipeline::review_issue(&context.env(), &mut prepared).unwrap();
    assert_eq!(
        (prepared.record.stage, prepared.record.status),
        (record::Stage::Ready, record::Status::Ok)
    );
    (spec, id)
}
