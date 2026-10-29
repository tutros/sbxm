//! A fake Docker Cloud Sandboxes for a whole cloud task (milestone 4), shared by the tests of
//! the commands that act on one after it ran: a copy of slice 5's helpers in
//! `tests/task_cloud_review.rs`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::task_fixture::{CLAUDE_DONE, CODEX_DONE, Fixture, is_headless, ok};
use sbxm::backend::{ExecOutput, FakeBackend, Location, SandboxInfo};
use sbxm::task::record::{self, Record};

/// The cloud sandbox's home, as `printenv HOME` reports it in the fake.
pub const HOME: &str = "/home/agent";

pub const CLEAN: &str = "Must-fix findings: 0\n\nRisk: low\n\nNothing found.\n";
pub const ONE: &str = "Must-fix findings: 1\n\nRisk: low\n\n## Must fix\n\n### M-1 - a.txt:1 does the \
                   wrong thing.\n";

/// The cloud minimum (P12).
pub const CLOUD_MIN: &str = "0.46.0";

pub fn identity() -> sbxm::task::repo::Identity {
    sbxm::task::repo::Identity {
        name: "Dev".into(),
        email: "dev@example.com".into(),
    }
}

/// The spec file every test starts from, and the task id it gives.
pub fn spec(f: &Fixture) -> (PathBuf, String) {
    let path = super::task_fixture::spec_file(f, "Build a thing.\n");
    let id = record::spec_id(&path).unwrap().0;
    (path, id)
}

pub fn worker_sandbox(id: &str) -> String {
    format!("sbxm-task-{id}-claude")
}

pub fn reviewer_sandbox(id: &str) -> String {
    format!("sbxm-task-{id}-review-codex")
}

/// A fake cloud that passes every preflight check, for a `claude` worker and a `codex` reviewer:
/// `<root>` stands in for every sandbox's filesystem, `printenv HOME` answers [`HOME`], and each
/// sandbox's own `sh`, `git` and `mkdir` commands really run on the host with their in-sandbox
/// paths moved under `<root>`. The worker's headless run (the first, then each fix round) commits
/// `a.txt` in its clone and writes `result.md`; the n-th reviewer run writes `reviews[n]` (the
/// last repeats) as `review.md` in its own clone; every other command (the gates) passes. The
/// worker sandbox is listed as running, as it is while the task runs.
pub fn cloud_backend(root: &Path, id: &str, reviews: &[&str]) -> FakeBackend {
    cloud_backend_listing(root, id, reviews, true)
}

/// [`cloud_backend`], with the worker sandbox listed as running (`listed`) or not: gone
/// (removed, or its TTL lapsed) by the time a fix round wants it. Each create of a sandbox that
/// was there before stands for a new sandbox, so what the old one's clone held is gone from the
/// fake's filesystem then.
pub fn cloud_backend_listing(root: &Path, id: &str, reviews: &[&str], listed: bool) -> FakeBackend {
    cloud_backend_for(root, &[id], reviews, listed)
}

/// The task id a sandbox of [`worker_sandbox`] or [`reviewer_sandbox`]'s name belongs to, and
/// whether it is the reviewer's.
pub fn sandbox_task(sandbox: &str) -> Option<(String, bool)> {
    let rest = sandbox.strip_prefix("sbxm-task-")?;
    if let Some(id) = rest.strip_suffix("-review-codex") {
        return Some((id.to_owned(), true));
    }
    Some((rest.strip_suffix("-claude")?.to_owned(), false))
}

/// [`cloud_backend_listing`] for the tasks `ids` at once (a `task run` list): each sandbox's name
/// says whose clone it works in.
pub fn cloud_backend_for(root: &Path, ids: &[&str], reviews: &[&str], listed: bool) -> FakeBackend {
    cloud_backend_worker(root, ids, reviews, listed, ok(CLAUDE_DONE))
}

/// [`cloud_backend_for`], with `worker` as what the worker's headless command gives back.
pub fn cloud_backend_worker(
    root: &Path,
    ids: &[&str],
    reviews: &[&str],
    listed: bool,
    worker: ExecOutput,
) -> FakeBackend {
    std::fs::create_dir_all(root.join(HOME.trim_start_matches('/'))).unwrap();
    let inside = root.to_string_lossy().into_owned();
    let map = move |arg: &str| arg.replace(HOME, &format!("{inside}{HOME}"));
    let (hook_map, fresh_map) = (map.clone(), map.clone());
    // The clone a sandbox of this name works in.
    let clone_of = |sandbox: &str| {
        sandbox_task(sandbox).map(|(id, review)| match review {
            true => format!("{HOME}/work/{id}-review"),
            false => format!("{HOME}/work/{id}"),
        })
    };
    let reviews: Vec<String> = reviews.iter().map(|r| (*r).to_owned()).collect();
    let (runs, reviewed) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let sandboxes = match listed {
        true => ids
            .iter()
            .map(|id| SandboxInfo {
                name: worker_sandbox(id),
                agent: "claude".into(),
                status: "running".into(),
            })
            .collect(),
        false => Vec::new(),
    };
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_create_hook(move |spec| {
            if let Some(clone) = clone_of(&spec.name).map(|c| PathBuf::from(fresh_map(&c)))
                && clone.exists()
            {
                std::fs::remove_dir_all(clone).unwrap();
            }
        })
        .with_version(CLOUD_MIN)
        .and_sandboxes(sandboxes)
        .with_sandbox_root(root)
        .with_exec_output_matching("printenv", ok(&format!("{HOME}\n")))
        .with_exec_output_matching("claude", worker)
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            let Some(clone) = clone_of(sandbox).map(|c| PathBuf::from(hook_map(&c))) else {
                return;
            };
            if !is_headless(spec) {
                return;
            }
            if !sandbox.contains("-review-") {
                let n = runs.fetch_add(1, Ordering::SeqCst);
                std::fs::write(clone.join("a.txt"), format!("run {n}\n")).unwrap();
                super::git(&clone, &["add", "-A"]);
                // A closing reference: the delivered branch's copy of this commit is rewritten
                // (issue 244), so it differs from the agent's own. Empty when a clone made
                // again from the agent's history already has this run's text.
                super::git(
                    &clone,
                    &[
                        "commit",
                        "-q",
                        "--allow-empty",
                        "-m",
                        &format!("run {n}\n\nFixes #9"),
                    ],
                );
                std::fs::write(clone.join(".sbxm-task").join("result.md"), "done\n").unwrap();
            } else {
                let n = reviewed.fetch_add(1, Ordering::SeqCst);
                let text = &reviews[n.min(reviews.len() - 1)];
                std::fs::write(clone.join(".sbxm-task").join("review.md"), text).unwrap();
            }
        })
        .with_exec_responder(move |_sandbox, spec| {
            if !matches!(spec.argv[0].as_str(), "sh" | "git" | "mkdir") {
                return None;
            }
            let mut command = std::process::Command::new(&spec.argv[0]);
            command.args(spec.argv[1..].iter().map(|a| map(a)));
            if let Some(dir) = &spec.workdir {
                command.current_dir(map(&dir.to_string_lossy()));
            }
            let out = command.output().unwrap();
            Some(ExecOutput {
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                exit_code: out.status.code(),
            })
        })
}

pub fn run_options(
    f: &Fixture,
    spec: PathBuf,
    location: Option<Location>,
) -> sbxm::commands::task_run::SpecOptions {
    sbxm::commands::task_run::SpecOptions {
        repo_root: f.env.tmp.path().join("target-repo"),
        spec,
        worker_harness: None,
        worker_model: None,
        reviewer_harness: None,
        reviewer_model: None,
        time_limit: None,
        reviewer_time_limit: None,
        profile: None,
        base: Some("main".into()),
        repo: Some("o/r".into()),
        clone_source: Some(super::task_fixture::source(f)),
        identity: Some(identity()),
        disk: Default::default(),
        location,
    }
}

/// What a command printed.
pub struct Ran {
    pub result: anyhow::Result<()>,
    pub out: String,
    pub warn: String,
}

/// `task run --spec FILE` with `location` (`None` leaves it to the config).
pub fn task_run(
    f: &Fixture,
    spec: PathBuf,
    location: Option<Location>,
    backend: &FakeBackend,
) -> Ran {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = sbxm::commands::task_run::run_spec(
        &f.env.config_dir(),
        &run_options(f, spec, location),
        None,
        backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &super::task_fixture::Probe,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    Ran {
        result,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
    }
}

pub fn task_record(f: &Fixture, id: &str) -> Record {
    record::read(&record::task_dir(&f.env.base_dir(), id).join("task.json")).unwrap()
}

/// The backend's state-changing calls in order, each `exec` named by what it ran.
pub fn described_log(backend: &FakeBackend) -> Vec<String> {
    let mut execs = backend.execs().into_iter();
    backend
        .log()
        .into_iter()
        .filter(|entry| !entry.starts_with("validate "))
        .map(|entry| {
            if !entry.starts_with("exec ") {
                return entry;
            }
            let (_, spec) = execs.next().unwrap();
            let argv = spec.argv.join(" ");
            let what = if argv.contains("printenv HOME") {
                "printenv HOME"
            } else if argv.contains("git clone") {
                "clone"
            } else if spec.argv[0] == "mkdir" {
                "mkdir"
            } else if argv.contains("claude") {
                "worker"
            } else if argv.contains("codex") {
                "reviewer"
            } else if argv.contains("bundle create") {
                "bundle"
            } else if argv.contains("status --porcelain") {
                "status"
            } else if argv.contains("cargo test") {
                "gate"
            } else {
                "other"
            };
            format!("exec {what}")
        })
        .collect()
}
