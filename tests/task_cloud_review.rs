//! Milestone 4 slice 5 (`sdlc/milestone-4.md`, work-order item 5; P5 (d), P14; decisions 253,
//! 256, 257, 261-263): reviews and fix rounds in the cloud, through `FakeBackend`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use common::task_fixture::{CLAUDE_DONE, CODEX_DONE, Fixture, fixture_with, is_headless, ok};
use sbxm::backend::{ExecOutput, FakeBackend, Location, SandboxInfo};
use sbxm::task::record::{self, Record, SandboxRole};

const LOCAL_TOML: &str = "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";

/// The cloud sandbox's home, as `printenv HOME` reports it in the fake.
const HOME: &str = "/home/agent";

const CLEAN: &str = "Must-fix findings: 0\n\nRisk: low\n\nNothing found.\n";

/// The cloud minimum (P12).
const CLOUD_MIN: &str = "0.46.0";

fn identity() -> sbxm::task::repo::Identity {
    sbxm::task::repo::Identity {
        name: "Dev".into(),
        email: "dev@example.com".into(),
    }
}

/// The spec file every test starts from, and the task id it gives.
fn spec(f: &Fixture) -> (PathBuf, String) {
    let path = common::task_fixture::spec_file(f, "Build a thing.\n");
    let id = record::spec_id(&path).unwrap().0;
    (path, id)
}

fn worker_sandbox(id: &str) -> String {
    format!("sbxm-task-{id}-claude")
}

fn reviewer_sandbox(id: &str) -> String {
    format!("sbxm-task-{id}-review-codex")
}

/// A fake cloud that passes every preflight check, for a `claude` worker and a `codex` reviewer:
/// `<root>` stands in for every sandbox's filesystem, `printenv HOME` answers [`HOME`], and each
/// sandbox's own `sh`, `git` and `mkdir` commands really run on the host with their in-sandbox
/// paths moved under `<root>`. The worker's headless run (the first, then each fix round) commits
/// `a.txt` in its clone and writes `result.md`; the n-th reviewer run writes `reviews[n]` (the
/// last repeats) as `review.md` in its own clone; every other command (the gates) passes. The
/// worker sandbox is listed as running, as it is while the task runs.
fn cloud_backend(root: &Path, id: &str, reviews: &[&str]) -> FakeBackend {
    cloud_backend_listing(root, id, reviews, true)
}

/// [`cloud_backend`], with the worker sandbox listed as running (`listed`) or not.
fn cloud_backend_listing(root: &Path, id: &str, reviews: &[&str], listed: bool) -> FakeBackend {
    std::fs::create_dir_all(root.join(HOME.trim_start_matches('/'))).unwrap();
    let inside = root.to_string_lossy().into_owned();
    let map = move |arg: &str| arg.replace(HOME, &format!("{inside}{HOME}"));
    let hook_map = map.clone();
    let worker = worker_sandbox(id);
    let clone = format!("{HOME}/work/{id}");
    let review_clone = format!("{HOME}/work/{id}-review");
    let reviews: Vec<String> = reviews.iter().map(|r| (*r).to_owned()).collect();
    let (runs, reviewed) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let sandboxes = if listed {
        vec![SandboxInfo {
            name: worker.clone(),
            agent: "claude".into(),
            status: "running".into(),
        }]
    } else {
        Vec::new()
    };
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_version(CLOUD_MIN)
        .and_sandboxes(sandboxes)
        .with_sandbox_root(root)
        .with_exec_output_matching("printenv", ok(&format!("{HOME}\n")))
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            if !is_headless(spec) {
                return;
            }
            if sandbox == worker {
                let n = runs.fetch_add(1, Ordering::SeqCst);
                let clone = PathBuf::from(hook_map(&clone));
                std::fs::write(clone.join("a.txt"), format!("run {n}\n")).unwrap();
                common::git(&clone, &["add", "-A"]);
                common::git(&clone, &["commit", "-q", "-m", &format!("run {n}")]);
                std::fs::write(clone.join(".sbxm-task").join("result.md"), "done\n").unwrap();
            } else if sandbox.contains("-review-") {
                let n = reviewed.fetch_add(1, Ordering::SeqCst);
                let dir = PathBuf::from(hook_map(&review_clone)).join(".sbxm-task");
                std::fs::write(dir.join("review.md"), &reviews[n.min(reviews.len() - 1)]).unwrap();
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

fn run_options(
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
        clone_source: Some(common::task_fixture::source(f)),
        identity: Some(identity()),
        disk: Default::default(),
        location,
    }
}

/// What a command printed.
struct Ran {
    result: anyhow::Result<()>,
    out: String,
    warn: String,
}

/// `task run --spec FILE` with `location` (`None` leaves it to the config).
fn task_run(f: &Fixture, spec: PathBuf, location: Option<Location>, backend: &FakeBackend) -> Ran {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = sbxm::commands::task_run::run_spec(
        &f.env.config_dir(),
        &run_options(f, spec, location),
        None,
        backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &common::task_fixture::Probe,
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

fn task_record(f: &Fixture, id: &str) -> Record {
    record::read(&record::task_dir(&f.env.base_dir(), id).join("task.json")).unwrap()
}

/// The backend's state-changing calls in order, each `exec` named by what it ran.
fn described_log(backend: &FakeBackend) -> Vec<String> {
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

/// The calls a cloud sandbox's seeding makes: `$HOME`, the bundle in, the clone, its
/// `.sbxm-task` folder and the files copied into it.
fn seeded(sandbox: &str, clone: &str, bundle: &str, files: &[&str]) -> Vec<String> {
    let mut calls = vec![
        format!("create {sandbox}"),
        "exec printenv HOME".to_owned(),
        format!("cp-in {sandbox} {HOME}/.sbxm-task-{bundle}.bundle"),
        "exec clone".to_owned(),
        "exec mkdir".to_owned(),
    ];
    calls.extend(
        files
            .iter()
            .map(|name| format!("cp-in {sandbox} {clone}/.sbxm-task/{name}")),
    );
    calls
}

/// The worker's run and its collection.
fn worked(sandbox: &str, clone: &str) -> Vec<String> {
    vec![
        "exec worker".to_owned(),
        "exec bundle".to_owned(),
        format!("cp-out {sandbox} {clone}/.sbxm-task/branch.bundle"),
        "exec status".to_owned(),
        format!("cp-out {sandbox} {clone}/.sbxm-task/result.md"),
    ]
}

/// One review round in its own cloud sandbox, removed after it.
fn review_round(id: &str, files: &[&str]) -> Vec<String> {
    let sandbox = reviewer_sandbox(id);
    let clone = format!("{HOME}/work/{id}-review");
    let mut calls = seeded(&sandbox, &clone, &format!("{id}-review"), files);
    calls.extend([
        "exec reviewer".to_owned(),
        format!("cp-out {sandbox} {clone}/.sbxm-task/review.md"),
        format!("rm {sandbox}"),
    ]);
    calls
}

/// Asserts every lifetime entry is stamped removed, with the roles in order.
fn assert_all_removed(task: &Record, roles: &[SandboxRole]) {
    let got: Vec<SandboxRole> = task.sandboxes.iter().map(|s| s.role).collect();
    assert_eq!(got, roles, "{:?}", task.sandboxes);
    for entry in &task.sandboxes {
        assert!(entry.removed_at.is_some(), "{entry:?}");
        assert_eq!(entry.location, Location::Cloud, "{entry:?}");
    }
}

/// Done when, and the first boundary case: a cloud `task run --spec` whose first review finds 0
/// must-fix reaches `ready` with one reviewer sandbox created and removed, and the worker sandbox
/// removed at ready; every lifetime entry has a `removed_at`.
#[test]
fn a_cloud_run_whose_first_review_is_clean_is_ready_with_every_sandbox_removed() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let root = f.env.tmp.path().join("cloud-fs");
    let backend = cloud_backend(&root, &id, &[CLEAN]);
    let (worker, reviewer) = (worker_sandbox(&id), reviewer_sandbox(&id));
    let clone = format!("{HOME}/work/{id}");

    let ran = task_run(&f, spec, Some(Location::Cloud), &backend);

    ran.result.unwrap();
    let mut expected = seeded(&worker, &clone, &id, &["source.md", "prompt.md"]);
    expected.extend(worked(&worker, &clone));
    expected.push("exec gate".to_owned());
    expected.extend(review_round(&id, &["source.md", "prompt.md"]));
    expected.push(format!("rm {worker}"));
    assert_eq!(described_log(&backend), expected, "{}", ran.out);

    let task = task_record(&f, &id);
    assert_eq!(task.stage, record::Stage::Ready);
    assert_eq!(task.location, Location::Cloud);
    assert_all_removed(&task, &[SandboxRole::Worker, SandboxRole::Reviewer]);
    // The reviewer's sandbox: the reviewer TTL (slice 3), the reviewer profile's two kits and no
    // workspace or template (decision 254).
    let create = &backend.creates()[1];
    assert_eq!(create.name, reviewer);
    assert_eq!(
        create.ttl,
        Some(sbxm::task::lifetime::reviewer_ttl(&f.config).unwrap())
    );
    assert_eq!(create.workspace, None);
    assert_eq!(create.template, None);
    let kits: Vec<_> = create
        .kits
        .iter()
        .map(|k| k.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(kits, ["common", "harness-codex"]);
    // The reviewer ran in its own clone, and its review came out by copy.
    let review_clone = format!("{HOME}/work/{id}-review");
    for (sandbox, spec) in backend.execs() {
        if sandbox == reviewer && spec.argv.iter().any(|a| a == "codex") {
            assert_eq!(spec.workdir.as_deref(), Some(Path::new(&review_clone)));
        }
    }
    assert_eq!(task.reviewer.as_ref().unwrap().workspace, review_clone);
    let meta = record::task_dir(&f.env.base_dir(), &id);
    assert!(
        std::fs::read_to_string(meta.join("review.md"))
            .unwrap()
            .contains("Must-fix findings: 0")
    );
    // One line at each create and each removal, for `run.log` (decision 265); no host clone,
    // and every staging folder gone.
    let ttl = sbxm::backend::ttl_arg(sbxm::task::lifetime::reviewer_ttl(&f.config).unwrap());
    for line in [
        format!("cloud sandbox {reviewer} created (reviewer, TTL {ttl})"),
        format!("cloud sandbox {reviewer} removed"),
        format!("cloud sandbox {worker} removed"),
    ] {
        assert_eq!(ran.out.matches(&line).count(), 1, "{line}\n{}", ran.out);
    }
    assert!(ran.out.contains("ready"), "{}", ran.out);
    assert!(ran.warn.is_empty(), "{}", ran.warn);
    assert!(!f.env.base_dir().join("tasks").join(&id).exists());
    assert!(
        !f.env
            .base_dir()
            .join("tasks")
            .join(format!("{id}-review"))
            .exists()
    );
    let staged = std::fs::read_dir(meta.join("staging")).map_or(0, |d| d.count());
    assert_eq!(staged, 0);
}

fn pr_7() -> sbxm::github::PrInfo {
    sbxm::github::PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        head_oid: "0000000000000000000000000000000000000000".into(),
        is_cross_repository: false,
        state: sbxm::github::PrState::Open,
        title: "Add the x feature".into(),
        body: "Adds x.\n\nFixes #4".into(),
        closing_issues: vec![4],
    }
}

fn pr_options(f: &Fixture, location: Option<Location>) -> sbxm::commands::task_review::Options {
    sbxm::commands::task_review::Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        target: sbxm::commands::task_review::Target::Pr(7),
        repo: Some("o/r".into()),
        base: Some("main".into()),
        clone_source: Some(common::task_fixture::source(f)),
        reviewer_harness: None,
        reviewer_model: None,
        reviewer_time_limit: None,
        time_limit: None,
        profile: None,
        resolved_spec_id: None,
        force: false,
        disk: Default::default(),
        location,
        identity: None,
    }
}

/// `task review` with `opts`.
fn task_review(
    f: &Fixture,
    opts: &sbxm::commands::task_review::Options,
    backend: &FakeBackend,
    github: &sbxm::github::fake::FakeGitHub,
) -> Ran {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = sbxm::commands::task_review::run(
        &f.env.config_dir(),
        opts,
        backend,
        github,
        &common::task_fixture::Probe,
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

/// Behavior 6: `task review --pr N --cloud` bundles the PR's head from `repo.git` into a reviewer
/// sandbox of its own, runs the sandbox gates and the reviewer there, removes the sandbox and
/// posts the review as today.
#[test]
fn a_cloud_pr_review_runs_in_a_reviewer_sandbox_and_posts_the_review() {
    let f = fixture_with(LOCAL_TOML);
    common::task_fixture::add_pr_head(&f, 7);
    let root = f.env.tmp.path().join("cloud-fs");
    let backend = cloud_backend(&root, "pr-7", &[CLEAN]);
    let github = sbxm::github::fake::FakeGitHub::default()
        .with_pr(pr_7())
        .with_issue_text(common::task_fixture::issue_text(4));

    let ran = task_review(
        &f,
        &pr_options(&f, Some(Location::Cloud)),
        &backend,
        &github,
    );

    ran.result.unwrap();
    let sandbox = reviewer_sandbox("pr-7");
    let clone = format!("{HOME}/work/pr-7-review");
    let mut expected = seeded(&sandbox, &clone, "pr-7-review", &["issue.md", "prompt.md"]);
    expected.extend([
        "exec gate".to_owned(),
        "exec reviewer".to_owned(),
        format!("cp-out {sandbox} {clone}/.sbxm-task/review.md"),
        format!("rm {sandbox}"),
    ]);
    assert_eq!(described_log(&backend), expected, "{}", ran.out);
    // The sandbox gates ran in the reviewer's clone.
    let gate = backend
        .execs()
        .into_iter()
        .find(|(_, spec)| spec.argv.join(" ").contains("cargo test"))
        .unwrap();
    assert_eq!(gate.1.workdir.as_deref(), Some(Path::new(&clone)));

    let task = task_record(&f, "pr-7");
    assert_eq!(task.stage, record::Stage::Ready);
    assert_eq!(task.location, Location::Cloud);
    assert_all_removed(&task, &[SandboxRole::Reviewer]);
    assert!(
        github
            .calls()
            .iter()
            .any(|c| matches!(c, sbxm::github::fake::GhCall::PrComment { .. })),
        "{:?}",
        github.calls()
    );
    assert!(ran.out.contains("was posted on PR #7"), "{}", ran.out);
    assert_eq!(
        ran.out
            .matches(&format!("cloud sandbox {sandbox} removed"))
            .count(),
        1,
        "{}",
        ran.out
    );
    assert!(!f.env.base_dir().join("tasks").join("pr-7-review").exists());
}
