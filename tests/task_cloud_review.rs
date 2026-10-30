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
const ONE: &str = "Must-fix findings: 1\n\nRisk: low\n\n## Must fix\n\n### M-1 - a.txt:1 does the \
                   wrong thing.\n";

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

/// [`cloud_backend`], with the worker sandbox listed as running (`listed`) or not: gone
/// (removed, or its TTL lapsed) by the time a fix round wants it. Each create of a sandbox that
/// was there before stands for a new sandbox, so what the old one's clone held is gone from the
/// fake's filesystem then.
fn cloud_backend_listing(root: &Path, id: &str, reviews: &[&str], listed: bool) -> FakeBackend {
    cloud_backend_for(root, &[id], reviews, listed)
}

/// The task id a sandbox of [`worker_sandbox`] or [`reviewer_sandbox`]'s name belongs to, and
/// whether it is the reviewer's.
fn sandbox_task(sandbox: &str) -> Option<(String, bool)> {
    let rest = sandbox.strip_prefix("sbxm-task-")?;
    if let Some(id) = rest.strip_suffix("-review-codex") {
        return Some((id.to_owned(), true));
    }
    Some((rest.strip_suffix("-claude")?.to_owned(), false))
}

/// [`cloud_backend_listing`] for the tasks `ids` at once (a `task run` list): each sandbox's name
/// says whose clone it works in.
fn cloud_backend_for(root: &Path, ids: &[&str], reviews: &[&str], listed: bool) -> FakeBackend {
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
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
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
                common::git(&clone, &["add", "-A"]);
                // A closing reference: the delivered branch's copy of this commit is rewritten
                // (issue 244), so it differs from the agent's own.
                common::git(
                    &clone,
                    &["commit", "-q", "-m", &format!("run {n}\n\nFixes #9")],
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

/// A fix round in the worker's clone: its input copied in, the worker, and its collection.
fn fixed(sandbox: &str, clone: &str) -> Vec<String> {
    let mut calls = vec![
        format!("cp-in {sandbox} {clone}/.sbxm-task/review.md"),
        format!("cp-in {sandbox} {clone}/.sbxm-task/fix-prompt.md"),
    ];
    calls.extend(worked(sandbox, clone));
    calls
}

/// The start of a cloud `task run --spec`: the worker's sandbox, its run and its gates.
fn started(id: &str) -> Vec<String> {
    let (worker, clone) = (worker_sandbox(id), format!("{HOME}/work/{id}"));
    let mut calls = seeded(&worker, &clone, id, &["source.md", "prompt.md"]);
    calls.extend(worked(&worker, &clone));
    calls.push("exec gate".to_owned());
    calls
}

const LATER_ROUND: [&str; 3] = ["source.md", "prompt.md", "previous-review.md"];

/// One fix round, then nothing must be fixed: a reviewer sandbox for each round (the narrow
/// review after the fix finds nothing, so a full one confirms it, spec §5.2), and the fix round in
/// the worker sandbox the run kept, which is removed at ready.
#[test]
fn one_fix_round_reuses_the_worker_sandbox_and_each_review_gets_its_own() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id, &[ONE, CLEAN]);
    let (worker, clone) = (worker_sandbox(&id), format!("{HOME}/work/{id}"));

    let ran = task_run(&f, spec, Some(Location::Cloud), &backend);

    ran.result.unwrap();
    let mut expected = started(&id);
    expected.extend(review_round(&id, &["source.md", "prompt.md"]));
    expected.extend(fixed(&worker, &clone));
    expected.push("exec gate".to_owned());
    expected.extend(review_round(&id, &LATER_ROUND));
    expected.extend(review_round(&id, &LATER_ROUND));
    expected.push(format!("rm {worker}"));
    assert_eq!(described_log(&backend), expected, "{}", ran.out);
    // The fix round ran in the worker's recorded clone.
    let fix = backend
        .execs()
        .into_iter()
        .filter(|(sandbox, spec)| *sandbox == worker && is_headless(spec))
        .nth(1)
        .unwrap();
    assert_eq!(fix.1.workdir.as_deref(), Some(Path::new(&clone)));

    let task = task_record(&f, &id);
    assert_eq!((task.stage, task.round), (record::Stage::Ready, 1));
    assert_eq!(task.review.as_ref().unwrap().must_fix, 0);
    assert_all_removed(
        &task,
        &[
            SandboxRole::Worker,
            SandboxRole::Reviewer,
            SandboxRole::Reviewer,
            SandboxRole::Reviewer,
        ],
    );
    // The fix round's commit is in `repo.git`, past the worker's.
    let meta = record::task_dir(&f.env.base_dir(), &id);
    let log = common::git(
        &meta.join("repo.git"),
        &["log", "--format=%s", &format!("refs/heads/{id}")],
    );
    assert_eq!(log.lines().take(2).collect::<Vec<_>>(), ["run 1", "run 0"]);
    assert_eq!(
        ran.out
            .matches(&format!("cloud sandbox {worker} removed"))
            .count(),
        1,
        "{}",
        ran.out
    );
}

/// The worker sandbox gone before a fix round: made again (role `fix`) from the recorded profile
/// and a bundle of the agent's own branch from `repo.git`, and the fix round's commits come back
/// as for the worker's (a fast-forward of the agent's ref, though the delivered branch's commits
/// were rewritten); removed at ready, and the gone sandbox's entry is closed.
#[test]
fn a_worker_sandbox_gone_before_a_fix_round_is_made_again_and_removed_at_ready() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let root = f.env.tmp.path().join("cloud-fs");
    let backend = cloud_backend_listing(&root, &id, &[ONE, CLEAN], false);
    let (worker, clone) = (worker_sandbox(&id), format!("{HOME}/work/{id}"));

    let ran = task_run(&f, spec, Some(Location::Cloud), &backend);

    ran.result.unwrap();
    let mut expected = started(&id);
    expected.extend(review_round(&id, &["source.md", "prompt.md"]));
    expected.extend(seeded(&worker, &clone, &id, &["source.md", "prompt.md"]));
    expected.extend(fixed(&worker, &clone));
    expected.push("exec gate".to_owned());
    expected.extend(review_round(&id, &LATER_ROUND));
    expected.extend(review_round(&id, &LATER_ROUND));
    expected.push(format!("rm {worker}"));
    assert_eq!(described_log(&backend), expected, "{}", ran.out);

    let task = task_record(&f, &id);
    assert_eq!((task.stage, task.round), (record::Stage::Ready, 1));
    assert_all_removed(
        &task,
        &[
            SandboxRole::Worker,
            SandboxRole::Reviewer,
            SandboxRole::Fix,
            SandboxRole::Reviewer,
            SandboxRole::Reviewer,
        ],
    );
    // Made again as slice 1 makes it: the worker TTL, the recorded profile's two kits.
    let rebuilt = &backend.creates()[2];
    assert_eq!(rebuilt.name, worker);
    assert_eq!(
        rebuilt.ttl,
        Some(sbxm::task::lifetime::worker_ttl(&f.config, f.config.fix_rounds).unwrap())
    );
    assert_eq!(rebuilt.workspace, None);
    let ttl = sbxm::backend::ttl_arg(
        sbxm::task::lifetime::worker_ttl(&f.config, f.config.fix_rounds).unwrap(),
    );
    for line in [
        format!("cloud sandbox {worker} is gone; it is made again for the fix round"),
        format!("cloud sandbox {worker} created (fix, TTL {ttl})"),
        format!("cloud sandbox {worker} removed"),
    ] {
        assert_eq!(ran.out.matches(&line).count(), 1, "{line}\n{}", ran.out);
    }
    let meta = record::task_dir(&f.env.base_dir(), &id);
    let log = common::git(
        &meta.join("repo.git"),
        &["log", "--format=%s", &format!("refs/heads/{id}")],
    );
    assert_eq!(log.lines().take(2).collect::<Vec<_>>(), ["run 1", "run 0"]);
}

/// The fix-round budget used with a must-fix finding left: the task stops `ready` (rounds
/// exhausted) and every sandbox it made is removed.
#[test]
fn rounds_exhausted_with_a_finding_left_is_ready_with_every_sandbox_removed() {
    let f = fixture_with(&format!("{LOCAL_TOML}\n[worker]\nfix_rounds = 1\n"));
    let (spec, id) = spec(&f);
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id, &[ONE]);
    let worker = worker_sandbox(&id);

    let ran = task_run(&f, spec, Some(Location::Cloud), &backend);

    ran.result.unwrap();
    let task = task_record(&f, &id);
    assert_eq!(task.stage, record::Stage::Ready);
    assert_eq!(task.stopped, Some(record::Stopped::RoundsExhausted));
    assert_eq!(task.review.as_ref().unwrap().must_fix, 1);
    assert_all_removed(
        &task,
        &[
            SandboxRole::Worker,
            SandboxRole::Reviewer,
            SandboxRole::Reviewer,
        ],
    );
    assert_eq!(backend.log().last(), Some(&format!("rm {worker}")));
    assert!(ran.out.contains("must-fix finding(s) left"), "{}", ran.out);
}

const CLOUD_TOML: &str = "[sandbox]\nprofile = \"default\"\nlocation = \"cloud\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";

fn spec_review_options(f: &Fixture, spec: PathBuf) -> sbxm::commands::task_review::Options {
    let mut opts = pr_options(f, None);
    opts.target = sbxm::commands::task_review::Target::Spec(spec);
    opts.identity = Some(identity());
    opts
}

/// `task start --spec FILE --cloud` alone: the task waits for its review at `gating/passed`, its
/// worker sandbox kept.
fn start_cloud(f: &Fixture, spec: PathBuf, backend: &FakeBackend) {
    sbxm::commands::task_start::run_spec(
        &f.env.config_dir(),
        &sbxm::commands::task_start::SpecOptions {
            repo_root: f.env.tmp.path().join("target-repo"),
            spec,
            worker_harness: None,
            worker_model: None,
            time_limit: None,
            reviewer_time_limit: None,
            profile: None,
            base: Some("main".into()),
            repo: Some("o/r".into()),
            clone_source: Some(common::task_fixture::source(f)),
            identity: Some(identity()),
            disk: Default::default(),
            location: Some(Location::Cloud),
        },
        backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &common::task_fixture::Probe,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap();
}

/// Decision 253: `task review --spec` takes no location flag; a task recorded `cloud` is reviewed
/// in the cloud though `sbxm-task.toml` says `local`.
#[test]
fn a_spec_review_of_a_task_recorded_cloud_runs_in_the_cloud_with_no_flag() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id, &[CLEAN]);
    start_cloud(&f, spec.clone(), &backend);

    let ran = task_review(
        &f,
        &spec_review_options(&f, spec),
        &backend,
        &sbxm::github::fake::FakeGitHub::default(),
    );

    ran.result.unwrap();
    assert_eq!(backend.locations(), [Location::Cloud, Location::Cloud]);
    let review = &backend.creates()[1];
    assert_eq!(review.name, reviewer_sandbox(&id));
    assert_eq!(review.workspace, None);
    let task = task_record(&f, &id);
    assert_eq!(task.stage, record::Stage::Ready);
    assert_all_removed(&task, &[SandboxRole::Worker, SandboxRole::Reviewer]);
}

/// Decision 253, the other way: a task recorded `local` is reviewed here though `sbxm-task.toml`
/// now says `cloud`.
#[test]
fn a_spec_review_of_a_task_recorded_local_runs_here_with_no_flag() {
    use common::task_fixture::{Play, play, play_reviews};

    let f = fixture_with(CLOUD_TOML);
    let (spec, id) = spec(&f);
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
    let reviewer = play_reviews(&f.env.base_dir(), &id, vec![CLEAN.to_owned()]);
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        });
    let github = sbxm::github::fake::FakeGitHub::default();
    let source = common::task_fixture::source(&f);
    let context = common::task_fixture::ctx(&f, &source, &backend, &github);
    let mut prepared = sbxm::task::pipeline::prepare_spec(&context, &spec).unwrap();
    sbxm::task::pipeline::run_worker(&context, &mut prepared).unwrap();

    let ran = task_review(&f, &spec_review_options(&f, spec), &backend, &github);

    ran.result.unwrap();
    assert_eq!(backend.locations(), [Location::Local]);
    let review = backend
        .creates()
        .into_iter()
        .find(|c| c.name == reviewer_sandbox(&id))
        .unwrap();
    assert!(
        review.workspace.is_some() && review.ttl.is_none(),
        "{review:?}"
    );
    let task = task_record(&f, &id);
    assert_eq!(
        (task.stage, task.location),
        (record::Stage::Ready, Location::Local)
    );
    assert!(task.sandboxes.is_empty(), "{:?}", task.sandboxes);
}

/// Behavior 2: a cloud `task run` preflights the reviewer's sandboxes too, before any write: a
/// `codex` reviewer whose `openai` login is missing in the cloud refuses the run, naming the
/// command that stores it, with no changing backend call and no task folder.
#[test]
fn a_codex_reviewer_without_its_cloud_login_refuses_a_cloud_run_before_any_write() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id, &[CLEAN])
        .and_secrets(&["anthropic"]);

    let ran = task_run(&f, spec, Some(Location::Cloud), &backend);

    let error = ran.result.unwrap_err();
    assert!(
        error.is::<sbxm::task::pipeline::MissingSecret>(),
        "{error:#}"
    );
    let message = format!("{error:#}");
    assert!(
        message.contains("'openai'")
            && message.contains("the reviewer, codex")
            && message.contains("`sbx secret set openai --oauth --cloud`")
            && message.ends_with("or run it here with `--local`"),
        "{message}"
    );
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(!f.env.base_dir().join(".sbxm").join("tasks").exists());
}

/// What a refused review must leave as it was: the task's record and the backend's calls.
struct Before {
    record: Vec<u8>,
    calls: usize,
}

fn before(f: &Fixture, id: &str, backend: &FakeBackend) -> Before {
    let meta = record::task_dir(&f.env.base_dir(), id);
    Before {
        record: std::fs::read(meta.join("task.json")).unwrap(),
        calls: backend.log().len(),
    }
}

fn assert_unchanged(f: &Fixture, id: &str, backend: &FakeBackend, was: &Before) {
    let meta = record::task_dir(&f.env.base_dir(), id);
    assert_eq!(std::fs::read(meta.join("task.json")).unwrap(), was.record);
    assert_eq!(
        backend.log()[was.calls..],
        [] as [String; 0],
        "{:?}",
        backend.log()
    );
}

/// Lesson 2 of the earlier run: a review that may make the worker sandbox again for a fix round
/// preflights that with the worker's recorded profile and harness, not the reviewer's. The worker
/// sandbox is gone and the cloud no longer holds the worker's own login: the review is refused
/// before it changes anything, though the reviewer's preflight passes.
#[test]
fn a_gone_worker_whose_recorded_profile_fails_preflight_refuses_the_review_before_any_change() {
    let f = fixture_with(LOCAL_TOML);
    f.env.write_profile("other", "");
    let (spec, id) = spec(&f);
    let backend = cloud_backend_listing(&f.env.tmp.path().join("cloud-fs"), &id, &[ONE], false);
    let mut start = run_options(&f, spec.clone(), Some(Location::Cloud));
    start.profile = Some("other".into());
    sbxm::commands::task_start::run_spec(
        &f.env.config_dir(),
        &sbxm::commands::task_start::SpecOptions {
            repo_root: start.repo_root,
            spec: spec.clone(),
            worker_harness: None,
            worker_model: None,
            time_limit: None,
            reviewer_time_limit: None,
            profile: start.profile,
            base: start.base,
            repo: start.repo,
            clone_source: start.clone_source,
            identity: start.identity,
            disk: Default::default(),
            location: start.location,
        },
        &backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &common::task_fixture::Probe,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap();
    // The worker's own login is gone from the cloud; the reviewer's (codex, `openai`) is still
    // there, so the reviewer's preflight passes and only the worker's fails.
    backend.set_secrets(&["openai"]);
    let was = before(&f, &id, &backend);

    let ran = task_review(
        &f,
        &spec_review_options(&f, spec),
        &backend,
        &sbxm::github::fake::FakeGitHub::default(),
    );

    let message = format!("{:#}", ran.result.unwrap_err());
    assert!(
        message.contains("'anthropic'") && message.contains("the worker, claude"),
        "{message}"
    );
    assert_unchanged(&f, &id, &backend, &was);
}

/// The same preflight covers the rebuild's own drift refusal (`ensure_worker_sandbox`'s rule):
/// the recorded profile still passes every cloud check, but its contents changed since the task
/// started, so the fix round's rebuild would refuse. The review says so before a reviewer
/// sandbox exists or `task.json` changes, instead of after a review it then can't act on.
#[test]
fn a_gone_worker_whose_recorded_profile_drifted_refuses_the_review_before_any_change() {
    let f = fixture_with(LOCAL_TOML);
    f.env.write_profile("other", "");
    let (spec, id) = spec(&f);
    let backend = cloud_backend_listing(&f.env.tmp.path().join("cloud-fs"), &id, &[ONE], false);
    let mut start = run_options(&f, spec.clone(), Some(Location::Cloud));
    start.profile = Some("other".into());
    sbxm::commands::task_start::run_spec(
        &f.env.config_dir(),
        &sbxm::commands::task_start::SpecOptions {
            repo_root: start.repo_root,
            spec: spec.clone(),
            worker_harness: None,
            worker_model: None,
            time_limit: None,
            reviewer_time_limit: None,
            profile: start.profile,
            base: start.base,
            repo: start.repo,
            clone_source: start.clone_source,
            identity: start.identity,
            disk: Default::default(),
            location: start.location,
        },
        &backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &common::task_fixture::Probe,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap();
    f.env
        .write_profile("other", "[network]\nallow = [\"crates.io\"]\n");
    let was = before(&f, &id, &backend);

    let ran = task_review(
        &f,
        &spec_review_options(&f, spec),
        &backend,
        &sbxm::github::fake::FakeGitHub::default(),
    );

    let message = format!("{:#}", ran.result.unwrap_err());
    assert!(
        message.contains("sandbox config changed") && message.contains("\"other\""),
        "{message}"
    );
    assert_unchanged(&f, &id, &backend, &was);
}

/// Lesson 3: a cloud review refuses a reviewer TTL that doesn't add up (slice 3's check) before
/// anything changes. (Its strict-profile case went with slice 1's refusal: slice 7 runs strict
/// reviewers in the cloud.)
#[test]
fn a_cloud_review_refuses_a_ttl_too_large_before_any_change() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id, &[CLEAN]);
    start_cloud(&f, spec.clone(), &backend);
    std::fs::write(
        f.env.tmp.path().join("target-repo").join("sbxm-task.toml"),
        format!("{LOCAL_TOML}\n[reviewer]\ntime_limit = \"{}s\"\n", u64::MAX),
    )
    .unwrap();
    let was = before(&f, &id, &backend);

    let ran = task_review(
        &f,
        &spec_review_options(&f, spec),
        &backend,
        &sbxm::github::fake::FakeGitHub::default(),
    );

    let message = format!("{:#}", ran.result.unwrap_err());
    assert!(message.contains("time_limit"), "{message}");
    assert_unchanged(&f, &id, &backend, &was);
}

fn list_options(f: &Fixture) -> sbxm::commands::task_run::ListOptions {
    sbxm::commands::task_run::ListOptions {
        repo_root: f.env.tmp.path().join("target-repo"),
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
        location: None,
    }
}

/// Lesson 1: a `task run` list of cloud targets resolves the location once for the list and skips
/// the local free-disk check for every target (P11), not only the first: both of two cloud spec
/// targets run to `ready` with too little local free disk.
#[test]
fn a_cloud_run_list_of_two_specs_with_too_little_local_disk_runs_both_to_ready() {
    use sbxm::commands::task_run::{ListOptions, Target, run_list};

    let f = fixture_with(CLOUD_TOML);
    let specs: Vec<PathBuf> = ["one.md", "two.md"]
        .iter()
        .map(|name| {
            let path = f.env.tmp.path().join(name);
            std::fs::write(&path, format!("Build {name}.\n")).unwrap();
            path
        })
        .collect();
    let ids: Vec<String> = specs
        .iter()
        .map(|p| record::spec_id(p).unwrap().0)
        .collect();
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let backend = cloud_backend_for(&f.env.tmp.path().join("cloud-fs"), &id_refs, &[CLEAN], true);
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    let result = run_list(
        &f.env.config_dir(),
        &ListOptions {
            disk: common::low_disk(&f.env),
            ..list_options(&f)
        },
        &specs.into_iter().map(Target::Spec).collect::<Vec<_>>(),
        None,
        &backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &common::task_fixture::Probe,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );

    let out = String::from_utf8(out).unwrap();
    result.unwrap();
    for id in &ids {
        let task = task_record(&f, id);
        assert_eq!(task.stage, record::Stage::Ready, "{id}\n{out}");
        assert_all_removed(&task, &[SandboxRole::Worker, SandboxRole::Reviewer]);
    }
    assert!(!out.contains(common::LOW_DISK), "{out}");
}

struct Yes;

impl sbxm::confirm::Confirm for Yes {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        Ok(true)
    }
}

/// `task run --spec FILE --restart` with `location`.
fn task_run_restart(
    f: &Fixture,
    spec: PathBuf,
    location: Option<Location>,
    backend: &FakeBackend,
) -> anyhow::Result<()> {
    sbxm::commands::task_run::run_spec(
        &f.env.config_dir(),
        &run_options(f, spec, location),
        Some(&sbxm::commands::task_start::Restart {
            confirm: &Yes,
            yes: true,
        }),
        backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &common::task_fixture::Probe,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut Vec::new(),
        &mut Vec::new(),
    )
}

/// Fetches task `id`'s result into the checkout, as `--restart` asks before discarding it.
fn fetch_result(f: &Fixture, id: &str) {
    let checkout = f.env.tmp.path().join("target-repo");
    if !checkout.join(".git").exists() {
        common::git(&checkout, &["init", "-q"]);
    }
    let repo_git = record::task_dir(&f.env.base_dir(), id).join("repo.git");
    common::git(
        &checkout,
        &[
            "fetch",
            "-q",
            repo_git.to_str().unwrap(),
            &format!("{id}:{id}"),
        ],
    );
}

/// Milestone 4 slice 6: `--restart` removes the old task's sandboxes where its record says they
/// are, whatever the config says: a cloud task whose worker sandbox's removal failed at its stop
/// has it removed in the cloud first, then starts again (here in the cloud, by the flag) and runs
/// to `ready` with a new record.
#[test]
fn a_restart_of_a_cloud_task_removes_its_open_sandbox_in_the_cloud_first() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let worker = worker_sandbox(&id);
    let root = f.env.tmp.path().join("cloud-fs");
    task_run(
        &f,
        spec.clone(),
        Some(Location::Cloud),
        &cloud_backend(&root, &id, &[CLEAN]),
    )
    .result
    .unwrap();
    let mut task = task_record(&f, &id);
    for entry in task.sandboxes.iter_mut().filter(|e| e.name == worker) {
        entry.removed_at = None;
    }
    record::write(&record::task_dir(&f.env.base_dir(), &id), &task).unwrap();
    fetch_result(&f, &id);
    let backend = cloud_backend(&root, &id, &[CLEAN]);

    task_run_restart(&f, spec, Some(Location::Cloud), &backend).unwrap();

    let log = backend.log();
    let first_rm = log.iter().position(|l| l.starts_with("rm ")).unwrap();
    let create = log.iter().position(|l| l.starts_with("create ")).unwrap();
    assert_eq!(log[first_rm], format!("rm {worker}"), "{log:?}");
    assert!(first_rm < create, "{log:?}");
    let task = task_record(&f, &id);
    assert_eq!(task.stage, record::Stage::Ready);
    assert_eq!(task.location, Location::Cloud);
    assert_all_removed(&task, &[SandboxRole::Worker, SandboxRole::Reviewer]);
}

/// The reverse: a task recorded `local` is restarted into the cloud, its old sandboxes removed
/// here (never in the cloud) before the new one is made in the cloud.
#[test]
fn a_restart_of_a_local_task_into_the_cloud_removes_it_here_first() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = common::task_fixture::ready_spec_task(&f);
    fetch_result(&f, &id);
    let worker = worker_sandbox(&id);
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id, &[CLEAN]);

    task_run_restart(&f, spec, Some(Location::Cloud), &backend).unwrap();

    let log = backend.log();
    let first_rm = log.iter().position(|l| l.starts_with("rm ")).unwrap();
    let create = log.iter().position(|l| l.starts_with("create ")).unwrap();
    assert_eq!(log[first_rm], format!("rm {worker}"), "{log:?}");
    assert!(first_rm < create, "{log:?}");
    // The command selects the cloud for the new task first; the removal of the old one selects
    // the recorded location after that, and nothing else selects the local backend.
    let locations = backend.locations();
    let first_cloud = locations
        .iter()
        .position(|l| *l == Location::Cloud)
        .unwrap();
    let local = locations.iter().position(|l| *l == Location::Local);
    assert!(local.is_some_and(|i| i > first_cloud), "{locations:?}");
    assert_eq!(
        locations.iter().filter(|l| **l == Location::Local).count(),
        1,
        "{locations:?}"
    );
    let task = task_record(&f, &id);
    assert_eq!(task.location, Location::Cloud);
    assert_eq!(task.stage, record::Stage::Ready);
}

/// A fake scoped secret (slice 4): no production path fills one in while P8 (a) holds.
fn fake_secret() -> sbxm::backend::ScopedSecret {
    sbxm::backend::ScopedSecret {
        host: "postman-echo.com".into(),
        env: "P2_FAKE".into(),
        value: "fake-scoped-value-NOT-REAL".into(),
    }
}

/// Behavior 3: a reviewer sandbox gets the task's scoped secrets before its create, and after its
/// round it is removed with slice 3's retry (here once "temporarily unavailable", then gone after
/// 5 s), then its scoped secrets; the worker sandbox and its secrets go at ready.
#[test]
fn a_reviewer_sandbox_gets_the_scoped_secrets_and_its_removal_is_retried_then_they_go() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let (worker, reviewer) = (worker_sandbox(&id), reviewer_sandbox(&id));
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id, &[CLEAN])
        .with_remove_errors(&[&format!(
            "`sbx rm` failed for sandbox {reviewer} (exit status: 1): error: sandbox {reviewer} \
             is temporarily unavailable; retry the delete"
        )]);
    let secrets = [fake_secret()];
    sbxm::commands::task_start::run_spec_with_seams(
        &f.env.config_dir(),
        &sbxm::commands::task_start::SpecOptions {
            repo_root: f.env.tmp.path().join("target-repo"),
            spec,
            worker_harness: None,
            worker_model: None,
            time_limit: None,
            reviewer_time_limit: None,
            profile: None,
            base: Some("main".into()),
            repo: Some("o/r".into()),
            clone_source: Some(common::task_fixture::source(&f)),
            identity: Some(identity()),
            disk: Default::default(),
            location: Some(Location::Cloud),
        },
        &backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &common::task_fixture::Probe,
        &mut Vec::new(),
        &mut Vec::new(),
        &|_| {},
        &secrets,
    )
    .unwrap();
    let started = backend.log().len();
    let mut prepared = sbxm::task::pipeline::Prepared::open(&f.env.base_dir(), &id).unwrap();
    let waits = std::sync::Mutex::new(Vec::new());
    let who = identity();
    let env = sbxm::task::pipeline::TaskEnv {
        auth: Box::leak(Box::new(sbxm::auth::Host::real())),
        config_dir: &f.env.config_dir(),
        config: &f.config,
        backend: &backend,
        probe: &common::task_fixture::Probe,
        host: &sbxm::task::gates::FakeHostRunner::default(),
        sleep: &|d| waits.lock().unwrap().push(d),
        scoped_secrets: &secrets,
        identity: Some(&who),
    };

    sbxm::task::pipeline::review_issue(&env, &mut prepared).unwrap();

    let calls: Vec<String> = backend.log()[started..]
        .iter()
        .filter(|c| c.starts_with("secret-") || c.starts_with("create ") || c.starts_with("rm "))
        .cloned()
        .collect();
    assert_eq!(
        calls,
        [
            format!("secret-set {reviewer} postman-echo-com"),
            format!("create {reviewer}"),
            format!("rm {reviewer}"),
            format!("rm {reviewer}"),
            format!("secret-ls {reviewer}"),
            format!("secret-rm {reviewer} postman-echo-com"),
            format!("rm {worker}"),
            format!("secret-ls {worker}"),
            format!("secret-rm {worker} postman-echo-com"),
        ]
    );
    assert_eq!(
        waits.into_inner().unwrap(),
        [std::time::Duration::from_secs(5)]
    );
    assert!(
        backend.cloud_secrets().is_empty(),
        "{:?}",
        backend.cloud_secrets()
    );
    assert!(
        prepared.events.contains(&format!(
            "scoped secret postman-echo-com of {reviewer} removed"
        )),
        "{:?}",
        prepared.events
    );
    assert_all_removed(
        &prepared.record,
        &[SandboxRole::Worker, SandboxRole::Reviewer],
    );
}

/// Behavior 5: a review that fails stops the task: its reviewer sandbox is removed after the round
/// and its worker sandbox then, so a stopped cloud task has no sandbox left either.
#[test]
fn a_review_that_fails_removes_the_reviewer_and_the_worker_sandbox() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let backend = cloud_backend(
        &f.env.tmp.path().join("cloud-fs"),
        &id,
        &["no count on the first line\n"],
    );

    let ran = task_run(&f, spec, Some(Location::Cloud), &backend);

    let message = format!("{:#}", ran.result.unwrap_err());
    assert!(message.contains("Must-fix findings: <count>"), "{message}");
    let task = task_record(&f, &id);
    assert_eq!(
        (task.stage, task.status),
        (record::Stage::Reviewing, record::Status::Failed)
    );
    assert_all_removed(&task, &[SandboxRole::Worker, SandboxRole::Reviewer]);
    let worker = worker_sandbox(&id);
    assert_eq!(backend.log().last(), Some(&format!("rm {worker}")));
    assert!(
        ran.out.contains(&format!("cloud sandbox {worker} removed")),
        "{}",
        ran.out
    );
}

/// P14 and decision 253: a pull request's task that exists runs where its record says; a flag
/// that says otherwise is refused before anything changes, not quietly ignored.
#[test]
fn a_pr_review_flag_that_contradicts_the_recorded_location_is_refused_before_any_change() {
    let f = fixture_with(LOCAL_TOML);
    common::task_fixture::add_pr_head(&f, 7);
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), "pr-7", &[CLEAN]);
    let github = sbxm::github::fake::FakeGitHub::default()
        .with_pr(pr_7())
        .with_issue_text(common::task_fixture::issue_text(4));
    task_review(
        &f,
        &pr_options(&f, Some(Location::Cloud)),
        &backend,
        &github,
    )
    .result
    .unwrap();
    let was = before(&f, "pr-7", &backend);
    let asked = github.calls().len();

    let ran = task_review(
        &f,
        &pr_options(&f, Some(Location::Local)),
        &backend,
        &github,
    );

    let message = format!("{:#}", ran.result.unwrap_err());
    assert!(
        message.contains("in the cloud") && message.contains("--local"),
        "{message}"
    );
    assert_unchanged(&f, "pr-7", &backend, &was);
    assert_eq!(github.calls().len(), asked, "{:?}", github.calls());
}

/// The biggest `review.md` read (the pipeline's `REVIEW_CAP`).
const REVIEW_CAP: usize = 1024 * 1024;

/// Behavior 3: `review.md` comes out of the reviewer's sandbox into a fresh staging folder and is
/// read with slice 1's checks: at its cap it is read, one byte over it, or a link or a folder the
/// sandbox sent, is not used, and the staging folder is gone either way.
#[test]
fn a_staged_review_at_its_cap_is_read_and_one_over_it_a_link_or_a_folder_is_not() {
    use sbxm::backend::FakeCopy;

    let at_cap = {
        let mut text = CLEAN.as_bytes().to_vec();
        text.resize(REVIEW_CAP, b' ');
        text
    };
    let mut over = at_cap.clone();
    over.push(b' ');
    let cases = [
        (FakeCopy::File(at_cap), None),
        (FakeCopy::File(over), Some("larger than 1 MiB")),
        (
            FakeCopy::Link("/etc/passwd".into()),
            Some("not a plain file"),
        ),
        (FakeCopy::Folder, Some("not a plain file")),
    ];
    for (copy, refused) in cases {
        let f = fixture_with(LOCAL_TOML);
        let (spec, id) = spec(&f);
        let path = format!("{HOME}/work/{id}-review/.sbxm-task/review.md");
        let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id, &[CLEAN])
            .with_copy_out(&path, copy.clone());

        let ran = task_run(&f, spec, Some(Location::Cloud), &backend);

        let task = task_record(&f, &id);
        match refused {
            None => {
                ran.result.unwrap();
                assert_eq!(task.stage, record::Stage::Ready, "{copy:?}");
            }
            Some(why) => {
                let message = format!("{:#}", ran.result.unwrap_err());
                assert!(message.contains(why), "{copy:?}: {message}");
                assert_eq!(task.status, record::Status::Failed, "{copy:?}");
            }
        }
        assert_all_removed(&task, &[SandboxRole::Worker, SandboxRole::Reviewer]);
        let meta = record::task_dir(&f.env.base_dir(), &id);
        let staged = std::fs::read_dir(meta.join("staging")).map_or(0, |d| d.count());
        assert_eq!(staged, 0, "{copy:?}");
    }
}

// Slice 7: strict mode in the cloud (P9, decisions 255, 264). A strict reviewer sandbox and a
// rebuilt fix-round sandbox are held to their runtime lists like the worker's: the denies, one
// `run.log` line, the check, before any agent `exec`; failing closed the same way.

const ANTHROPIC_HOSTS: [&str; 3] = ["api.anthropic.com", "claude.ai", "console.anthropic.com"];
const OPENAI_HOSTS: [&str; 6] = [
    "api.openai.com",
    "openai.com",
    "auth.openai.com",
    "chatgpt.com",
    "files.openai.com",
    "releases.openai.com",
];

const STRICT_PROFILE: &str = "[template]\nstrict = true\nallow = [\"crates.io\"]\n";

/// What every sandbox of these tests starts out allowing: both providers' hosts, a kept host and
/// a build host.
fn strict_list() -> Vec<&'static str> {
    let mut list = ANTHROPIC_HOSTS.to_vec();
    list.extend(OPENAI_HOSTS);
    list.extend(["crates.io", "github.com"]);
    list
}

/// The calls that hold `sandbox` to its runtime list right after its create.
fn held(sandbox: &str, denied: &[&str]) -> Vec<String> {
    let mut calls = vec![format!("policy-ls {sandbox}")];
    calls.extend(denied.iter().map(|h| format!("policy-deny {sandbox} {h}")));
    calls.push(format!("policy-ls {sandbox}"));
    calls
}

/// The `run.log` line of a strict sandbox's denies.
fn denied_line(sandbox: &str, denied: &[&str]) -> String {
    format!(
        "strict cloud sandbox {sandbox}: sbxm denied {}",
        denied.join(", ")
    )
}

/// `log` with the calls of [`held`] for every create of a sandbox in `held_by` inserted right
/// after that create.
fn with_holds(log: Vec<String>, held_by: &[(&str, Vec<&str>)]) -> Vec<String> {
    let mut out = Vec::new();
    for call in log {
        let hold = held_by
            .iter()
            .find(|(sandbox, _)| call == format!("create {sandbox}"));
        out.push(call);
        if let Some((sandbox, denied)) = hold {
            out.extend(held(sandbox, denied));
        }
    }
    out
}

/// A strict cloud `task run --spec` with a fix round in a rebuilt worker sandbox: the worker's,
/// each reviewer's and the fix round's sandbox each deny the hosts outside their own harness's
/// list (a `claude` worker keeps the Anthropic hosts, a `codex` reviewer the OpenAI ones, both
/// `[template].allow`), each says so in one line, and the run reaches ready.
#[test]
fn a_strict_cloud_run_holds_the_worker_each_reviewer_and_a_rebuilt_fix_sandbox() {
    let f = fixture_with(LOCAL_TOML);
    f.env.write_profile("default", STRICT_PROFILE);
    let (spec, id) = spec(&f);
    let root = f.env.tmp.path().join("cloud-fs");
    let backend =
        cloud_backend_listing(&root, &id, &[ONE, CLEAN], false).with_cloud_allow(&strict_list());
    let (worker, reviewer, clone) = (
        worker_sandbox(&id),
        reviewer_sandbox(&id),
        format!("{HOME}/work/{id}"),
    );

    let ran = task_run(&f, spec, Some(Location::Cloud), &backend);

    ran.result.unwrap();
    let mut worker_denies = OPENAI_HOSTS.to_vec();
    worker_denies.push("github.com");
    let mut reviewer_denies = ANTHROPIC_HOSTS.to_vec();
    reviewer_denies.push("github.com");
    let mut plain = started(&id);
    plain.extend(review_round(&id, &["source.md", "prompt.md"]));
    plain.extend(seeded(&worker, &clone, &id, &["source.md", "prompt.md"]));
    plain.extend(fixed(&worker, &clone));
    plain.push("exec gate".to_owned());
    plain.extend(review_round(&id, &LATER_ROUND));
    plain.extend(review_round(&id, &LATER_ROUND));
    plain.push(format!("rm {worker}"));
    let expected = with_holds(
        plain,
        &[
            (&worker, worker_denies.clone()),
            (&reviewer, reviewer_denies.clone()),
        ],
    );
    assert_eq!(described_log(&backend), expected, "{}", ran.out);
    // The worker's sandbox twice (its start and the fix round's rebuild), three reviewers.
    assert_eq!(
        ran.out
            .matches(&denied_line(&worker, &worker_denies))
            .count(),
        2,
        "{}",
        ran.out
    );
    assert_eq!(
        ran.out
            .matches(&denied_line(&reviewer, &reviewer_denies))
            .count(),
        3,
        "{}",
        ran.out
    );
    let task = task_record(&f, &id);
    assert_eq!((task.stage, task.round), (record::Stage::Ready, 1));
}

/// Starts a strict cloud task with the fake scoped secret and opens it for its review.
fn started_strict(
    f: &Fixture,
    backend: &FakeBackend,
    secrets: &[sbxm::backend::ScopedSecret],
) -> (String, sbxm::task::pipeline::Prepared) {
    let (spec, id) = spec(f);
    sbxm::commands::task_start::run_spec_with_seams(
        &f.env.config_dir(),
        &sbxm::commands::task_start::SpecOptions {
            repo_root: f.env.tmp.path().join("target-repo"),
            spec,
            worker_harness: None,
            worker_model: None,
            time_limit: None,
            reviewer_time_limit: None,
            profile: None,
            base: Some("main".into()),
            repo: Some("o/r".into()),
            clone_source: Some(common::task_fixture::source(f)),
            identity: Some(identity()),
            disk: Default::default(),
            location: Some(Location::Cloud),
        },
        backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &common::task_fixture::Probe,
        &mut Vec::new(),
        &mut Vec::new(),
        &|_| {},
        secrets,
    )
    .unwrap();
    let prepared = sbxm::task::pipeline::Prepared::open(&f.env.base_dir(), &id).unwrap();
    (id, prepared)
}

/// The fail-closed path for a strict reviewer sandbox (behavior 4): a host left after the denies
/// removes the reviewer sandbox, then its scoped secret, and the review is refused naming the
/// host, with no `exec` in the reviewer's sandbox; the task stops, so its worker sandbox goes
/// too.
#[test]
fn a_strict_reviewer_with_a_host_left_is_removed_then_its_secret_then_refused() {
    let f = fixture_with(LOCAL_TOML);
    f.env.write_profile("default", STRICT_PROFILE);
    let probe_id = spec(&f).1;
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &probe_id, &[CLEAN])
        .with_undeniable_cloud_host_in("-review-", "api.x.ai")
        .with_cloud_allow(&{
            let mut list = strict_list();
            list.push("api.x.ai");
            list
        });
    let secrets = [fake_secret()];
    let (id, mut prepared) = started_strict(&f, &backend, &secrets);
    let (worker, reviewer) = (worker_sandbox(&id), reviewer_sandbox(&id));
    let started = backend.log().len();
    let who = identity();
    let env = sbxm::task::pipeline::TaskEnv {
        auth: Box::leak(Box::new(sbxm::auth::Host::real())),
        config_dir: &f.env.config_dir(),
        config: &f.config,
        backend: &backend,
        probe: &common::task_fixture::Probe,
        host: &sbxm::task::gates::FakeHostRunner::default(),
        sleep: &|_| {},
        scoped_secrets: &secrets,
        identity: Some(&who),
    };

    let result = sbxm::task::pipeline::review_issue(&env, &mut prepared);

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("api.x.ai") && message.contains(&reviewer),
        "{message}"
    );
    let mut denied = ANTHROPIC_HOSTS.to_vec();
    denied.extend(["github.com", "api.x.ai"]);
    let mut expected = vec![
        format!("secret-set {reviewer} postman-echo-com"),
        format!("create {reviewer}"),
    ];
    expected.extend(held(&reviewer, &denied));
    expected.extend([
        format!("rm {reviewer}"),
        format!("secret-ls {reviewer}"),
        format!("secret-rm {reviewer} postman-echo-com"),
        format!("rm {worker}"),
        format!("secret-ls {worker}"),
        format!("secret-rm {worker} postman-echo-com"),
    ]);
    let log: Vec<String> = backend.log()[started..]
        .iter()
        .filter(|c| !c.starts_with("validate "))
        .cloned()
        .collect();
    assert_eq!(log, expected);
    assert!(
        !backend
            .execs()
            .iter()
            .any(|(sandbox, _)| *sandbox == reviewer),
        "an exec ran in the reviewer's sandbox"
    );
    assert!(
        prepared.events.contains(&denied_line(&reviewer, &denied)),
        "{:?}",
        prepared.events
    );
    assert!(
        backend.cloud_secrets().is_empty(),
        "{:?}",
        backend.cloud_secrets()
    );
    assert_all_removed(
        &prepared.record,
        &[SandboxRole::Worker, SandboxRole::Reviewer],
    );
}

/// The fail-closed path for a rebuilt fix-round sandbox: its policy list can't be read, so it is
/// removed, then its scoped secret, and the fix round is refused naming the unread list, before
/// the fix round's worker runs.
#[test]
fn a_rebuilt_strict_fix_sandbox_whose_list_cannot_be_read_is_removed_then_refused() {
    let f = fixture_with(LOCAL_TOML);
    f.env.write_profile("default", STRICT_PROFILE);
    let probe_id = spec(&f).1;
    // Two reads for the worker's start and two for the first reviewer; the fifth, the rebuilt
    // fix sandbox's first, fails.
    let backend =
        cloud_backend_listing(&f.env.tmp.path().join("cloud-fs"), &probe_id, &[ONE], false)
            .with_cloud_allow(&strict_list())
            .failing_cloud_policy_ls_from(4);
    let secrets = [fake_secret()];
    let (id, mut prepared) = started_strict(&f, &backend, &secrets);
    let worker = worker_sandbox(&id);
    let who = identity();
    let env = sbxm::task::pipeline::TaskEnv {
        auth: Box::leak(Box::new(sbxm::auth::Host::real())),
        config_dir: &f.env.config_dir(),
        config: &f.config,
        backend: &backend,
        probe: &common::task_fixture::Probe,
        host: &sbxm::task::gates::FakeHostRunner::default(),
        sleep: &|_| {},
        scoped_secrets: &secrets,
        identity: Some(&who),
    };
    let headless_before = backend
        .execs()
        .iter()
        .filter(|(s, spec)| *s == worker && is_headless(spec))
        .count();

    let result = sbxm::task::pipeline::review_issue(&env, &mut prepared);

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("can't read")
            && message.contains(&format!("sbx --cloud policy ls {worker} --json")),
        "{message}"
    );
    let log = backend.log();
    let rebuilt = log
        .iter()
        .rposition(|c| *c == format!("create {worker}"))
        .unwrap();
    // The gone sandbox's secret from the start is still listed with the new one's (the cloud's
    // `rm` leaves them, P2 point 6), so the list read removes both.
    assert_eq!(
        log[rebuilt - 1..],
        [
            format!("secret-set {worker} postman-echo-com"),
            format!("create {worker}"),
            format!("policy-ls {worker}"),
            format!("rm {worker}"),
            format!("secret-ls {worker}"),
            format!("secret-rm {worker} postman-echo-com"),
            format!("secret-rm {worker} postman-echo-com"),
        ]
    );
    assert_eq!(
        backend
            .execs()
            .iter()
            .filter(|(s, spec)| *s == worker && is_headless(spec))
            .count(),
        headless_before,
        "the fix round's worker ran"
    );
    assert!(
        backend.cloud_secrets().is_empty(),
        "{:?}",
        backend.cloud_secrets()
    );
    let roles: Vec<SandboxRole> = prepared.record.sandboxes.iter().map(|s| s.role).collect();
    assert_eq!(
        roles,
        [SandboxRole::Worker, SandboxRole::Reviewer, SandboxRole::Fix]
    );
    assert!(
        prepared
            .record
            .sandboxes
            .iter()
            .all(|s| s.removed_at.is_some()),
        "{:?}",
        prepared.record.sandboxes
    );
}
