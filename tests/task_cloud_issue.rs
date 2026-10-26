//! Milestone 4 slice 5b (`sdlc/milestone-4.md`, the Commands table, P14, P15; decisions 253,
//! 261, 262): issue tasks in the cloud, on the same cloud path a `--spec` task takes, through
//! `FakeBackend` and `FakeGitHub`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, fixture_with, is_headless, issue_text, ok, open_issue,
    play_tasks,
};
use sbxm::backend::{ExecOutput, FakeBackend, Location, SandboxInfo};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::record::{self, Record, SandboxRole};

const LOCAL_TOML: &str = "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";
const CLOUD_TOML: &str = "[sandbox]\nprofile = \"default\"\nlocation = \"cloud\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";

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

fn worker_sandbox(id: &str) -> String {
    format!("sbxm-task-{id}-claude")
}

fn reviewer_sandbox(id: &str) -> String {
    format!("sbxm-task-{id}-review-codex")
}

/// Issues 41 and 42, open, with their text.
fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_issue_text(issue_text(42))
        .with_open_issues(vec![
            open_issue(41, &["should-fix"], ""),
            open_issue(42, &["should-fix"], ""),
        ])
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

/// A fake cloud that passes every preflight check, for a `claude` worker and a `codex` reviewer
/// of the tasks `ids`, as `tests/task_cloud_review.rs` has it: `<root>` stands in for every
/// sandbox's filesystem, `printenv HOME` answers [`HOME`], and each sandbox's own `sh`, `git` and
/// `mkdir` commands really run on the host with their in-sandbox paths moved under `<root>`. The
/// worker's headless run commits `a.txt` in its clone and writes `result.md`; a reviewer's
/// writes [`CLEAN`]; every other command (the gates) passes. The worker sandboxes are listed as
/// running.
fn cloud_backend(root: &Path, ids: &[&str]) -> FakeBackend {
    cloud_backend_with(root, ids, ok(CLAUDE_DONE))
}

/// [`cloud_backend`], with `worker` as what the worker's headless command gives back.
fn cloud_backend_with(root: &Path, ids: &[&str], worker: ExecOutput) -> FakeBackend {
    std::fs::create_dir_all(root.join(HOME.trim_start_matches('/'))).unwrap();
    let inside = root.to_string_lossy().into_owned();
    let map = move |arg: &str| arg.replace(HOME, &format!("{inside}{HOME}"));
    let (hook_map, fresh_map) = (map.clone(), map.clone());
    let clone_of = |sandbox: &str| {
        sandbox_task(sandbox).map(|(id, review)| match review {
            true => format!("{HOME}/work/{id}-review"),
            false => format!("{HOME}/work/{id}"),
        })
    };
    let runs = AtomicUsize::new(0);
    let sandboxes = ids
        .iter()
        .map(|id| SandboxInfo {
            name: worker_sandbox(id),
            agent: "claude".into(),
            status: "running".into(),
        })
        .collect();
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
                common::git(&clone, &["add", "-A"]);
                common::git(&clone, &["commit", "-q", "-m", &format!("run {n}")]);
                std::fs::write(clone.join(".sbxm-task").join("result.md"), "done\n").unwrap();
            } else {
                std::fs::write(clone.join(".sbxm-task").join("review.md"), CLEAN).unwrap();
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

/// A local fake that plays the worker in its host clone and a clean reviewer in its own.
fn local_backend(f: &Fixture) -> FakeBackend {
    let worker = play_tasks(
        &f.env.base_dir(),
        "main",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let base_dir = f.env.base_dir();
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            if let Some((id, true)) = sandbox_task(sandbox)
                && is_headless(spec)
            {
                let dir = base_dir
                    .join("tasks")
                    .join(format!("{id}-review"))
                    .join(".sbxm-task");
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("review.md"), CLEAN).unwrap();
            }
        })
}

fn task_record(f: &Fixture, id: &str) -> Record {
    record::read(&record::task_dir(&f.env.base_dir(), id).join("task.json")).unwrap()
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

/// What a command printed.
struct Ran {
    result: anyhow::Result<()>,
    out: String,
    warn: String,
}

fn run_options(
    f: &Fixture,
    issue: u32,
    location: Option<Location>,
) -> sbxm::commands::task_run::Options {
    sbxm::commands::task_run::Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        issue,
        worker_harness: None,
        worker_model: None,
        reviewer_harness: None,
        reviewer_model: None,
        time_limit: None,
        reviewer_time_limit: None,
        profile: None,
        base: None,
        repo: Some("o/r".into()),
        clone_source: Some(common::task_fixture::source(f)),
        identity: Some(identity()),
        disk: Default::default(),
        location,
    }
}

/// `task run --issue N` with `location` (`None` leaves it to the config).
fn task_run(
    f: &Fixture,
    opts: &sbxm::commands::task_run::Options,
    backend: &FakeBackend,
    github: &FakeGitHub,
) -> Ran {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = sbxm::commands::task_run::run(
        &f.env.config_dir(),
        opts,
        None,
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

fn start_options(
    f: &Fixture,
    issues: Vec<u32>,
    workers: Option<usize>,
    location: Option<Location>,
) -> sbxm::commands::task_start::Options {
    sbxm::commands::task_start::Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        issues,
        workers,
        worker_harness: None,
        worker_model: None,
        time_limit: None,
        profile: None,
        base: None,
        repo: Some("o/r".into()),
        clone_source: Some(common::task_fixture::source(f)),
        identity: Some(identity()),
        disk: Default::default(),
        location,
    }
}

fn task_start(
    f: &Fixture,
    opts: &sbxm::commands::task_start::Options,
    backend: &FakeBackend,
    github: &FakeGitHub,
) -> Ran {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = sbxm::commands::task_start::run_with(
        &f.env.config_dir(),
        opts,
        None,
        backend,
        github,
        &common::task_fixture::Probe,
        &mut out,
        &mut warn,
    );
    Ran {
        result,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
    }
}

fn review_options(f: &Fixture, issue: u32) -> sbxm::commands::task_review::Options {
    sbxm::commands::task_review::Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        target: sbxm::commands::task_review::Target::Issue(issue),
        repo: Some("o/r".into()),
        base: None,
        clone_source: Some(common::task_fixture::source(f)),
        reviewer_harness: None,
        reviewer_model: None,
        reviewer_time_limit: None,
        time_limit: None,
        profile: None,
        resolved_spec_id: None,
        force: false,
        disk: Default::default(),
        location: None,
        identity: Some(identity()),
    }
}

fn task_review(f: &Fixture, issue: u32, backend: &FakeBackend, github: &FakeGitHub) -> Ran {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = sbxm::commands::task_review::run(
        &f.env.config_dir(),
        &review_options(f, issue),
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

/// The first boundary case, by the flag and by the config: `task run --issue N` in the cloud
/// reaches `ready` with its worker and reviewer sandboxes created in the cloud and removed, the
/// issue's text as the worker's input, and no host clone.
#[test]
fn a_cloud_issue_run_reaches_ready_with_its_worker_and_reviewer_sandboxes_removed() {
    for (toml, location) in [(LOCAL_TOML, Some(Location::Cloud)), (CLOUD_TOML, None)] {
        let f = fixture_with(toml);
        let id = "issue-41";
        let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &[id]);
        let (worker, reviewer) = (worker_sandbox(id), reviewer_sandbox(id));

        let ran = task_run(&f, &run_options(&f, 41, location), &backend, &github());

        ran.result.expect(&ran.out);
        let task = task_record(&f, id);
        assert_eq!(task.stage, record::Stage::Ready, "{}", ran.out);
        assert_eq!(task.location, Location::Cloud);
        assert_eq!(task.review.as_ref().unwrap().must_fix, 0);
        assert_all_removed(&task, &[SandboxRole::Worker, SandboxRole::Reviewer]);
        assert!(backend.locations().iter().all(|l| *l == Location::Cloud));
        let creates: Vec<_> = backend.creates().into_iter().map(|c| c.name).collect();
        assert_eq!(creates, [worker.clone(), reviewer.clone()]);
        for create in backend.creates() {
            assert_eq!(create.workspace, None);
            assert!(create.ttl.is_some());
        }
        let log = backend.log();
        assert!(log.contains(&format!("rm {worker}")), "{log:?}");
        assert!(log.contains(&format!("rm {reviewer}")), "{log:?}");
        // The issue's text went in as the worker's `issue.md`.
        let clone = format!("{HOME}/work/{id}");
        assert!(
            log.contains(&format!("cp-in {worker} {clone}/.sbxm-task/issue.md")),
            "{log:?}"
        );
        assert!(!f.env.base_dir().join("tasks").join(id).exists());
        for line in [
            format!("cloud sandbox {worker} removed"),
            format!("cloud sandbox {reviewer} removed"),
        ] {
            assert_eq!(ran.out.matches(&line).count(), 1, "{line}\n{}", ran.out);
        }
        assert!(
            ran.out.contains("next: sbxm task finish --issue 41"),
            "{}",
            ran.out
        );
        assert!(ran.warn.is_empty(), "{}", ran.warn);
    }
}

/// P14: `task run --issue N --local` over a cloud `[sandbox] location` runs here, to `ready`, with
/// no cloud call.
#[test]
fn a_local_flag_over_a_cloud_config_runs_an_issue_here_to_ready() {
    let f = fixture_with(CLOUD_TOML);
    let backend = local_backend(&f);

    let ran = task_run(
        &f,
        &run_options(&f, 41, Some(Location::Local)),
        &backend,
        &github(),
    );

    ran.result.expect(&ran.out);
    let task = task_record(&f, "issue-41");
    assert_eq!(task.stage, record::Stage::Ready);
    assert_eq!(task.location, Location::Local);
    assert!(task.sandboxes.is_empty(), "{:?}", task.sandboxes);
    assert!(
        !backend.locations().contains(&Location::Cloud),
        "{:?}",
        backend.locations()
    );
}

/// Decision 253: `task review --issue N` takes no location flag; a task recorded `cloud` is
/// reviewed in the cloud though `sbxm-task.toml` says `local`.
#[test]
fn an_issue_review_of_a_task_recorded_cloud_runs_in_the_cloud_with_no_flag() {
    let f = fixture_with(LOCAL_TOML);
    let id = "issue-41";
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &[id]);
    let gh = github();
    let started = task_start(
        &f,
        &start_options(&f, vec![41], None, Some(Location::Cloud)),
        &backend,
        &gh,
    );
    started.result.expect(&started.out);
    // The start hands over to the review with its worker sandbox kept.
    let task = task_record(&f, id);
    assert_eq!(task.location, Location::Cloud);
    assert_eq!(task.sandboxes.len(), 1);
    assert!(task.sandboxes[0].removed_at.is_none());
    assert!(
        started.out.contains("next: sbxm task review --issue 41"),
        "{}",
        started.out
    );

    let ran = task_review(&f, 41, &backend, &gh);

    ran.result.expect(&ran.out);
    assert_eq!(backend.locations(), [Location::Cloud, Location::Cloud]);
    let review = &backend.creates()[1];
    assert_eq!(review.name, reviewer_sandbox(id));
    assert_eq!(review.workspace, None);
    let task = task_record(&f, id);
    assert_eq!(task.stage, record::Stage::Ready);
    assert_all_removed(&task, &[SandboxRole::Worker, SandboxRole::Reviewer]);
}

/// Decision 253, the other way: a task recorded `local` is reviewed here though `sbxm-task.toml`
/// now says `cloud`.
#[test]
fn an_issue_review_of_a_task_recorded_local_runs_here_with_no_flag() {
    let f = fixture_with(LOCAL_TOML);
    let backend = local_backend(&f);
    let gh = github();
    let started = task_start(&f, &start_options(&f, vec![41], None, None), &backend, &gh);
    started.result.expect(&started.out);
    std::fs::write(
        f.env.tmp.path().join("target-repo").join("sbxm-task.toml"),
        CLOUD_TOML,
    )
    .unwrap();

    let ran = task_review(&f, 41, &backend, &gh);

    ran.result.expect(&ran.out);
    assert!(
        !backend.locations().contains(&Location::Cloud),
        "{:?}",
        backend.locations()
    );
    let task = task_record(&f, "issue-41");
    assert_eq!(task.stage, record::Stage::Ready);
    assert_eq!(task.location, Location::Local);
}

/// P15 (a): `task start --workers 2` in the cloud gives each worker its own cloud sandbox; both
/// are removed at their stops (here, `ready` after each one's review).
#[test]
fn two_cloud_workers_get_a_sandbox_each_and_both_are_removed_at_their_stops() {
    let f = fixture_with(CLOUD_TOML);
    let ids = ["issue-41", "issue-42"];
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &ids);
    let gh = github();

    let started = task_start(
        &f,
        &start_options(&f, Vec::new(), Some(2), None),
        &backend,
        &gh,
    );

    started.result.expect(&started.out);
    let mut creates: Vec<_> = backend.creates().into_iter().map(|c| c.name).collect();
    creates.sort();
    assert_eq!(creates, [worker_sandbox(ids[0]), worker_sandbox(ids[1])]);
    for id in ids {
        let task = task_record(&f, id);
        assert_eq!(task.location, Location::Cloud);
        assert_eq!(task.sandboxes.len(), 1, "{id}");
        let created = format!("cloud sandbox {} created (worker", worker_sandbox(id));
        assert_eq!(started.out.matches(&created).count(), 1, "{}", started.out);
    }

    for (number, id) in [(41, ids[0]), (42, ids[1])] {
        let ran = task_review(&f, number, &backend, &gh);
        ran.result.expect(&ran.out);
        let task = task_record(&f, id);
        assert_eq!(task.stage, record::Stage::Ready);
        assert_all_removed(&task, &[SandboxRole::Worker, SandboxRole::Reviewer]);
    }
    assert!(backend.locations().iter().all(|l| *l == Location::Cloud));
}

/// P15 (a), the other half: every worker is preflighted before any is created, so a preflight
/// failure (no cloud `anthropic` login) creates neither sandbox and no task folder.
#[test]
fn a_cloud_preflight_failure_with_two_workers_creates_neither() {
    let f = fixture_with(CLOUD_TOML);
    let backend = FakeBackend::with_secrets(&["openai"]).with_version(CLOUD_MIN);

    let ran = task_start(
        &f,
        &start_options(&f, Vec::new(), Some(2), None),
        &backend,
        &github(),
    );

    let message = format!("{:#}", ran.result.unwrap_err());
    assert!(
        message.contains("anthropic") && message.contains("cloud"),
        "{message}"
    );
    assert!(backend.creates().is_empty(), "{:?}", backend.log());
    assert!(
        !backend.log().iter().any(|e| e.starts_with("secret-set")),
        "{:?}",
        backend.log()
    );
    assert!(!record::task_dir(&f.env.base_dir(), "issue-41").exists());
    assert!(!record::task_dir(&f.env.base_dir(), "issue-42").exists());
}

/// A cloud worker that fails stops its issue task, and its sandbox is removed then, as a spec
/// task's is.
#[test]
fn a_failed_cloud_issue_worker_stops_and_its_sandbox_is_removed() {
    let f = fixture_with(CLOUD_TOML);
    let id = "issue-41";
    let failed = ExecOutput {
        stdout: String::new(),
        stderr: "boom\n".into(),
        exit_code: Some(3),
    };
    let backend = cloud_backend_with(&f.env.tmp.path().join("cloud-fs"), &[id], failed);

    let ran = task_start(
        &f,
        &start_options(&f, vec![41], None, None),
        &backend,
        &github(),
    );

    assert!(ran.result.is_err(), "{}", ran.out);
    let task = task_record(&f, id);
    assert_all_removed(&task, &[SandboxRole::Worker]);
    assert_eq!(
        backend.log().last(),
        Some(&format!("rm {}", worker_sandbox(id)))
    );
    assert!(
        ran.out
            .contains(&format!("cloud sandbox {} removed", worker_sandbox(id))),
        "{}",
        ran.out
    );
}

/// A cloud issue start that fails after its create (the sandbox's `HOME` unusable) removes the
/// sandbox it made, says so, and leaves no task folder, as a spec start does.
#[test]
fn a_cloud_issue_start_failing_after_its_create_removes_the_sandbox_and_the_folder() {
    let f = fixture_with(CLOUD_TOML);
    let id = "issue-41";
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_version(CLOUD_MIN)
        .with_exec_output_matching("printenv", ok("relative\n"));

    let ran = task_start(
        &f,
        &start_options(&f, vec![41], None, None),
        &backend,
        &github(),
    );

    assert!(ran.result.is_err(), "{}", ran.out);
    assert!(ran.out.contains("HOME"), "{}", ran.out);
    assert_eq!(
        backend.log().last(),
        Some(&format!("rm {}", worker_sandbox(id)))
    );
    assert!(
        ran.out
            .contains(&format!("cloud sandbox {} removed", worker_sandbox(id))),
        "{}",
        ran.out
    );
    assert!(!record::task_dir(&f.env.base_dir(), id).exists());
}

/// Behavior 4: a `task run` list of one cloud issue and one cloud spec target resolves the
/// location once and runs both in the cloud to `ready`.
#[test]
fn a_cloud_list_of_an_issue_and_a_spec_runs_both_to_ready() {
    use sbxm::commands::task_run::{ListOptions, Target, run_list};

    let f = fixture_with(CLOUD_TOML);
    let spec = common::task_fixture::spec_file(&f, "Build a thing.\n");
    let spec_id = record::spec_id(&spec).unwrap().0;
    let ids = ["issue-41", spec_id.as_str()];
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &ids);
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    let result = run_list(
        &f.env.config_dir(),
        &ListOptions {
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
            clone_source: Some(common::task_fixture::source(&f)),
            identity: Some(identity()),
            disk: Default::default(),
            location: None,
        },
        &[Target::Issue(41), Target::Spec(spec)],
        None,
        &backend,
        &github(),
        &common::task_fixture::Probe,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );

    let out = String::from_utf8(out).unwrap();
    result.expect(&out);
    for id in ids {
        let task = task_record(&f, id);
        assert_eq!(task.stage, record::Stage::Ready, "{id}\n{out}");
        assert_eq!(task.location, Location::Cloud);
        assert_all_removed(&task, &[SandboxRole::Worker, SandboxRole::Reviewer]);
    }
    assert!(backend.locations().iter().all(|l| *l == Location::Cloud));
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

/// As for a spec task: `--restart` removes the old task's sandboxes, which follows a task's
/// recorded location only from slice 6 on, so `task run --issue N --restart` and `task start
/// --restart` into the cloud, or of an issue task recorded `cloud`, are refused before anything,
/// and the old task stays as it was.
#[test]
fn an_issue_restart_into_the_cloud_or_of_a_cloud_issue_task_is_refused_before_anything() {
    let f = fixture_with(LOCAL_TOML);
    let id = "issue-41";
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &[id]);
    let gh = github();
    let restart = sbxm::commands::task_start::Restart {
        confirm: &Yes,
        yes: true,
    };
    let run_restart = |location| {
        sbxm::commands::task_run::run(
            &f.env.config_dir(),
            &run_options(&f, 41, location),
            Some(&restart),
            &backend,
            &gh,
            &common::task_fixture::Probe,
            &sbxm::task::gates::FakeHostRunner::default(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
    };
    let start_restart = |location| {
        sbxm::commands::task_start::run_with(
            &f.env.config_dir(),
            &start_options(&f, vec![41], None, location),
            Some(&restart),
            &backend,
            &gh,
            &common::task_fixture::Probe,
            &mut Vec::new(),
            &mut Vec::new(),
        )
    };
    let refused = |result: anyhow::Result<()>| {
        let message = format!("{:#}", result.unwrap_err());
        assert!(
            message.contains("--restart") && message.contains("cloud"),
            "{message}"
        );
    };

    refused(run_restart(Some(Location::Cloud)));
    refused(start_restart(Some(Location::Cloud)));
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(gh.calls().is_empty(), "{:?}", gh.calls());
    assert!(!record::task_dir(&f.env.base_dir(), id).exists());

    let started = task_start(
        &f,
        &start_options(&f, vec![41], None, Some(Location::Cloud)),
        &backend,
        &gh,
    );
    started.result.expect(&started.out);
    let (log, record_was) = (backend.log(), task_record(&f, id));
    refused(run_restart(Some(Location::Local)));
    refused(start_restart(Some(Location::Local)));
    assert_eq!(backend.log(), log);
    assert_eq!(task_record(&f, id), record_was);
}
