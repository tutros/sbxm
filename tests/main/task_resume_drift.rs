//! Issue 410: `task resume` remakes a gone worker sandbox when what changed since the task started
//! is only its resources (`cpus`, `memory`) and/or its template image's tag. It says so on one
//! warning line naming each changed field old -> new, and records the new values in `task.json`.
//! Any other profile change is still refused, and the refusal names every changed field.

use crate::common;

use std::fs;
use std::path::PathBuf;

use common::task_fixture::{self, Fixture};
use sbxm::backend::{CreateSpec, FakeBackend};
use sbxm::commands::{task_resume, template_build};
use sbxm::harness::Harness;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::record;

const WORKER: &str = "sbxm-task-issue-41-claude";

/// A profile with a template, so the worker is created from one.
const PROFILE: &str = "[network]\n\
     allow = [\"archive.ubuntu.com\"]\n\
     \n\
     [[setup.install]]\n\
     command = \"apt-get install -y ripgrep\"\n\
     \n\
     [template]\n\
     allow = [\"crates.io\"]\n";

/// The tag of a template image an `sbx` state reset wiped.
const OLD_TAG: &str = "sbxm-default-claude:000000000000-20261103t0000";

/// Plays issue 41's worker (one commit) and a clean review, with `sbx` 0.46.0. The fake lists no
/// sandboxes, so a resumed worker's sandbox is always gone.
fn played(f: &Fixture) -> FakeBackend {
    let worker = task_fixture::play_tasks(
        &f.env.base_dir(),
        "main",
        task_fixture::Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = task_fixture::play_reviews(
        &f.env.base_dir(),
        "issue-41",
        vec!["Must-fix findings: 0\n\nNothing found.\n".to_owned()],
    );
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_version("0.46.0")
        .with_exec_output_matching("claude", task_fixture::ok(task_fixture::CLAUDE_DONE))
        .with_exec_output_matching("codex", task_fixture::ok(task_fixture::CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        })
}

/// Builds the template of `default` for claude and returns its tag.
fn build_template(f: &Fixture, backend: &FakeBackend) -> String {
    let mut out = Vec::new();
    template_build::run(
        &f.env.config_dir(),
        &template_build::Options {
            profile: "default".into(),
            harness: Harness::Claude,
            ..Default::default()
        },
        backend,
        &record::SystemProbe,
        &mut out,
        &mut Vec::new(),
    )
    .unwrap();
    String::from_utf8(out)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_owned()
}

/// Issue 41's worker created from the profile's template (cpus 4, memory 8g) and run once, then
/// left failed as a crash would leave it. Returns the template's tag.
fn failed_worker(f: &Fixture, backend: &FakeBackend) -> String {
    f.env.write_profile("default", PROFILE);
    let tag = build_template(f, backend);
    let mut prepared = task_fixture::worked_task(f, backend);
    let worker = prepared.record.worker.as_ref().unwrap();
    assert_eq!(worker.template.as_deref(), Some(tag.as_str()));
    assert_eq!(
        (worker.cpus, worker.memory.as_deref()),
        (Some(4), Some("8g"))
    );
    prepared.record.stage = record::Stage::Working;
    prepared.record.status = record::Status::Failed;
    record::write(&prepared.meta, &prepared.record).unwrap();
    tag
}

fn task_json(f: &Fixture) -> PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")
}

/// The global `[resources]` become `cpus = 2`, `memory = "4g"` (the host lost half its RAM).
fn halve_resources(f: &Fixture) {
    let path = f.env.config_dir().join("config.toml");
    let text = fs::read_to_string(&path).unwrap();
    let halved = text.replace("cpus = 4\nmemory = \"8g\"", "cpus = 2\nmemory = \"4g\"");
    assert_ne!(text, halved);
    fs::write(&path, halved).unwrap();
}

/// The task's worker sandbox was created from a template image that is gone now.
fn record_old_tag(f: &Fixture) {
    let mut task = record::read(&task_json(f)).unwrap();
    task.worker.as_mut().unwrap().template = Some(OLD_TAG.to_owned());
    record::write(task_json(f).parent().unwrap(), &task).unwrap();
}

/// The process a record names is gone (the `sbxm` that wrote it was killed).
struct Gone;

impl record::ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

fn resume_41(f: &Fixture, backend: &FakeBackend) -> (anyhow::Result<()>, String, String) {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_resume::run(
        &f.env.config_dir(),
        &task_resume::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            target: task_resume::Target::Issue(41),
            rounds: None,
            should_fix: false,
            disk: Default::default(),
            identity: None,
            forget_ambiguous: false,
        },
        backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &Gone,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    (
        result,
        String::from_utf8(out).unwrap(),
        String::from_utf8(warn).unwrap(),
    )
}

fn worker_create(backend: &FakeBackend) -> CreateSpec {
    backend
        .creates()
        .into_iter()
        .rfind(|c| c.name == WORKER)
        .expect("the worker's sandbox was made again")
}

/// The warning lines about the changed config.
fn changed_lines(warn: &str) -> Vec<&str> {
    warn.lines()
        .filter(|l| l.starts_with("warning: ") && l.contains("changed since it started"))
        .collect()
}

#[test]
fn task_resume_drift_only_resources_changed_resumes_with_one_warning_and_records_them() {
    let f = task_fixture::fixture();
    let backend = played(&f);
    let tag = failed_worker(&f, &backend);
    halve_resources(&f);

    let (result, _, warn) = resume_41(&f, &backend);

    result.unwrap();
    let create = worker_create(&backend);
    assert_eq!((create.cpus, create.memory.as_str()), (2, "4g"));
    assert_eq!(create.template.as_deref(), Some(tag.as_str()));
    let lines = changed_lines(&warn);
    assert_eq!(lines.len(), 1, "{warn}");
    assert!(lines[0].contains("cpus 4 -> 2"), "{warn}");
    assert!(lines[0].contains("memory 8g -> 4g"), "{warn}");
    assert!(!lines[0].contains("template"), "{warn}");
    let saved = record::read(&task_json(&f)).unwrap();
    let worker = saved.worker.as_ref().unwrap();
    assert_eq!(
        (worker.cpus, worker.memory.as_deref()),
        (Some(2), Some("4g"))
    );
    assert_eq!(saved.stage, record::Stage::Ready);

    // The new values are the task's own now: resuming it again is no change.
    let mut task = saved;
    task.stage = record::Stage::Working;
    task.status = record::Status::Failed;
    record::write(task_json(&f).parent().unwrap(), &task).unwrap();
    let (result, _, warn) = resume_41(&f, &backend);
    result.unwrap();
    assert!(changed_lines(&warn).is_empty(), "{warn}");
    let create = worker_create(&backend);
    assert_eq!((create.cpus, create.memory.as_str()), (2, "4g"));
}

#[test]
fn task_resume_drift_only_the_template_tag_changed_resumes_with_one_warning_and_records_it() {
    let f = task_fixture::fixture();
    let backend = played(&f);
    let tag = failed_worker(&f, &backend);
    record_old_tag(&f);

    let (result, _, warn) = resume_41(&f, &backend);

    result.unwrap();
    assert_eq!(
        worker_create(&backend).template.as_deref(),
        Some(tag.as_str())
    );
    let lines = changed_lines(&warn);
    assert_eq!(lines.len(), 1, "{warn}");
    assert!(
        lines[0].contains(&format!("template {OLD_TAG} -> {tag}")),
        "{warn}"
    );
    assert!(!lines[0].contains("cpus"), "{warn}");
    assert!(!lines[0].contains("memory"), "{warn}");
    let saved = record::read(&task_json(&f)).unwrap();
    assert_eq!(
        saved.worker.unwrap().template.as_deref(),
        Some(tag.as_str())
    );
}

#[test]
fn task_resume_drift_resources_and_the_template_tag_changed_resume_with_one_warning() {
    let f = task_fixture::fixture();
    let backend = played(&f);
    let tag = failed_worker(&f, &backend);
    record_old_tag(&f);
    halve_resources(&f);

    let (result, _, warn) = resume_41(&f, &backend);

    result.unwrap();
    let lines = changed_lines(&warn);
    assert_eq!(lines.len(), 1, "{warn}");
    for field in [
        "cpus 4 -> 2".to_owned(),
        "memory 8g -> 4g".to_owned(),
        format!("template {OLD_TAG} -> {tag}"),
    ] {
        assert!(lines[0].contains(&field), "{field}: {warn}");
    }
    let saved = record::read(&task_json(&f)).unwrap();
    let worker = saved.worker.unwrap();
    assert_eq!(
        (worker.cpus, worker.memory.as_deref()),
        (Some(2), Some("4g"))
    );
    assert_eq!(worker.template.as_deref(), Some(tag.as_str()));
}

#[test]
fn task_resume_drift_resources_and_another_profile_change_are_refused_naming_every_field() {
    let f = task_fixture::fixture();
    let backend = played(&f);
    failed_worker(&f, &backend);
    halve_resources(&f);
    f.env.write_profile(
        "default",
        &PROFILE.replace("archive.ubuntu.com", "archive.ubuntu.com\", \"example.com"),
    );
    let before = fs::read(task_json(&f)).unwrap();
    let created = backend.creates().len();

    let (result, _, warn) = resume_41(&f, &backend);

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("changed since it started"), "{message}");
    for field in ["cpus 4 -> 2", "memory 8g -> 4g", "profile \"default\""] {
        assert!(message.contains(field), "{field}: {message}");
    }
    assert!(
        message.contains("sbxm task start --restart --issue 41"),
        "{message}"
    );
    assert!(changed_lines(&warn).is_empty(), "{warn}");
    assert_eq!(backend.creates().len(), created, "{:?}", backend.log());
    assert_eq!(fs::read(task_json(&f)).unwrap(), before);
}

#[test]
fn task_resume_drift_nothing_changed_resumes_without_a_warning() {
    let f = task_fixture::fixture();
    let backend = played(&f);
    let tag = failed_worker(&f, &backend);

    let (result, _, warn) = resume_41(&f, &backend);

    result.unwrap();
    assert!(changed_lines(&warn).is_empty(), "{warn}");
    let create = worker_create(&backend);
    assert_eq!((create.cpus, create.memory.as_str()), (4, "8g"));
    assert_eq!(create.template.as_deref(), Some(tag.as_str()));
}
