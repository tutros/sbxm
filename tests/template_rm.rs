//! Milestone 3 slice 7: `sbxm template rm` (decision 205(b)).

mod common;

use assert_cmd::Command;
use common::Env;
use sbxm::backend::{FakeBackend, SandboxBackend, TemplateImage};
use sbxm::commands::template_rm;
use sbxm::harness::Harness;
use sbxm::template::Record;

const NOW: u64 = 1_700_000_000;
const TAG: &str = "sbxm-default-claude:0123456789ab-20231114t2213";
const OLD: &str = "sbxm-default-claude:0123456789ab-20231113t1000";

fn image(reference: &str) -> TemplateImage {
    let (repository, tag) = reference.split_once(':').unwrap();
    TemplateImage {
        id: "sha256:1".into(),
        repository: format!("docker.io/library/{repository}"),
        tag: tag.into(),
        flavor: "claude-code-docker".into(),
        created_at: "2023-11-14T22:13:20Z".into(),
        size: 1_500_000_000,
    }
}

fn record_path(env: &Env, key: &str) -> std::path::PathBuf {
    env.base_dir()
        .join(".sbxm")
        .join("templates")
        .join(format!("{key}.json"))
}

/// Writes the record of `default-<harness>` naming `tag` and `previous`.
fn write_record(env: &Env, harness: Harness, tag: &str, previous: &[&str]) {
    Record {
        tag: tag.into(),
        template_hash: "0123456789ab".repeat(5) + "0123",
        warm: false,
        lock_files: Vec::new(),
        lock_hash: None,
        sbx_version: "0.46.0".into(),
        size_bytes: 1_500_000_000,
        created_at: "2023-11-14T22:13:20Z".into(),
        previous: previous.iter().map(|t| (*t).to_owned()).collect(),
    }
    .save(&env.base_dir(), &format!("default-{}", harness.as_str()))
    .unwrap();
}

fn backend_with(tags: &[&str]) -> FakeBackend {
    FakeBackend::default().with_templates(tags.iter().map(|t| image(t)).collect())
}

fn options(harness: Harness) -> template_rm::Options {
    template_rm::Options {
        profile: "default".into(),
        harness,
    }
}

fn rm(env: &Env, options: &template_rm::Options, backend: &FakeBackend) -> anyhow::Result<String> {
    let mut out = Vec::new();
    template_rm::run(&env.config_dir(), options, backend, &mut out)?;
    Ok(String::from_utf8(out).unwrap())
}

/// The refusal, after checking that no template was removed and the record is still there.
fn refused(env: &Env, options: &template_rm::Options, backend: &FakeBackend) -> String {
    let before = backend.template_ls().unwrap();
    let err = rm(env, options, backend).unwrap_err();
    assert!(
        !backend.log().iter().any(|l| l.starts_with("template-rm")),
        "{:?}",
        backend.log()
    );
    assert_eq!(backend.template_ls().unwrap(), before);
    format!("{err:#}")
}

/// Records `tag` as the template of project `demo`'s Claude sandbox.
fn sandbox_uses(env: &Env, tag: &str) {
    let mut state = sbxm::state::State::default();
    state.sandboxes.insert(
        "claude".into(),
        sbxm::state::SandboxState {
            sandbox: "sbxm-demo-claude".into(),
            workspace: env.base_dir().join("demo"),
            created_at: NOW,
            profile: Some("default".into()),
            config_hash: None,
            template: Some(tag.into()),
        },
    );
    state
        .save(&env.base_dir().join(".sbxm").join("demo"))
        .unwrap();
}

/// Writes a task whose reviewer records `tag`; returns the task's id.
fn task_uses(env: &Env, tag: &str) -> String {
    use sbxm::task::record::{self, Agent, Kind, NewTask, Process};
    let mut task = record::Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: 7,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        NOW,
        Process::new(1, NOW),
    );
    task.reviewer = Some(Agent {
        harness: "claude".into(),
        model: None,
        sandbox: "sbxm-task-7-review-claude".into(),
        workspace: "w".into(),
        profile: Some("default".into()),
        cpus: None,
        memory: None,
        template: Some(tag.into()),
        run: None,
    });
    record::write(&record::task_dir(&env.base_dir(), &task.id), &task).unwrap();
    task.id
}

#[test]
fn removes_the_template_and_its_record() {
    let env = Env::new();
    write_record(&env, Harness::Claude, TAG, &[]);
    let backend = backend_with(&[TAG]);

    let out = rm(&env, &options(Harness::Claude), &backend).unwrap();

    assert_eq!(backend.log(), vec![format!("template-rm {TAG}")]);
    assert!(backend.template_ls().unwrap().is_empty());
    assert!(!record_path(&env, "default-claude").exists());
    assert_eq!(out, format!("removed template {TAG}\n"));
}

#[test]
fn removes_the_previous_tags_the_record_kept() {
    let env = Env::new();
    write_record(&env, Harness::Claude, TAG, &[OLD]);
    let backend = backend_with(&[OLD, TAG]);

    rm(&env, &options(Harness::Claude), &backend).unwrap();

    assert!(backend.template_ls().unwrap().is_empty());
    assert!(!record_path(&env, "default-claude").exists());
}

#[test]
fn the_harness_flag_picks_that_harness_s_record() {
    let env = Env::new();
    let codex = "sbxm-default-codex:0123456789ab-20231114t2213";
    write_record(&env, Harness::Claude, TAG, &[]);
    write_record(&env, Harness::Codex, codex, &[]);
    let backend = backend_with(&[TAG, codex]);

    rm(&env, &options(Harness::Codex), &backend).unwrap();

    assert_eq!(backend.log(), vec![format!("template-rm {codex}")]);
    assert!(record_path(&env, "default-claude").exists());
    assert!(!record_path(&env, "default-codex").exists());
}

#[test]
fn refused_while_a_sandbox_uses_it() {
    let env = Env::new();
    write_record(&env, Harness::Claude, TAG, &[]);
    sandbox_uses(&env, TAG);
    let backend = backend_with(&[TAG]);

    let message = refused(&env, &options(Harness::Claude), &backend);

    assert!(
        message.contains(&format!(
            "template {TAG} is in use by sandbox sbxm-demo-claude; remove those first"
        )),
        "{message}"
    );
    assert!(record_path(&env, "default-claude").exists());
}

#[test]
fn refused_while_a_task_uses_it() {
    let env = Env::new();
    write_record(&env, Harness::Claude, TAG, &[]);
    let id = task_uses(&env, TAG);
    let backend = backend_with(&[TAG]);

    let message = refused(&env, &options(Harness::Claude), &backend);

    assert!(
        message.contains(&format!("in use by task {id}")),
        "{message}"
    );
}

#[test]
fn refused_while_a_previous_tag_is_in_use() {
    let env = Env::new();
    write_record(&env, Harness::Claude, TAG, &[OLD]);
    sandbox_uses(&env, OLD);
    let backend = backend_with(&[OLD, TAG]);

    let message = refused(&env, &options(Harness::Claude), &backend);

    assert!(
        message.contains(&format!(
            "template {OLD} is in use by sandbox sbxm-demo-claude"
        )),
        "{message}"
    );
}

#[test]
fn refused_when_a_task_record_cannot_be_read() {
    let env = Env::new();
    write_record(&env, Harness::Claude, TAG, &[]);
    let id = task_uses(&env, "something-else");
    let path = sbxm::task::record::task_dir(&env.base_dir(), &id).join("task.json");
    std::fs::write(&path, "not json").unwrap();
    let backend = backend_with(&[TAG]);

    refused(&env, &options(Harness::Claude), &backend);

    assert!(record_path(&env, "default-claude").exists());
}

#[test]
fn refused_without_a_record_before_any_sbx_call() {
    let env = Env::new();
    let backend = backend_with(&[TAG]);

    let message = refused(&env, &options(Harness::Claude), &backend);

    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(
        message.contains(
            "profile 'default' has no template for claude; `sbxm template ls` lists the templates"
        ),
        "{message}"
    );
}

#[test]
fn an_invalid_profile_name_is_refused() {
    let env = Env::new();
    let backend = backend_with(&[]);

    let message = refused(
        &env,
        &template_rm::Options {
            profile: "../x".into(),
            harness: Harness::Claude,
        },
        &backend,
    );

    assert!(message.contains("invalid profile name '../x'"), "{message}");
    assert!(backend.log().is_empty());
}

#[test]
fn an_image_already_gone_only_loses_its_record() {
    let env = Env::new();
    write_record(&env, Harness::Claude, TAG, &[]);
    let backend = backend_with(&[]);

    rm(&env, &options(Harness::Claude), &backend).unwrap();

    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(!record_path(&env, "default-claude").exists());
}

#[test]
fn a_failed_removal_keeps_the_record() {
    let env = Env::new();
    write_record(&env, Harness::Claude, TAG, &[]);
    let backend = backend_with(&[TAG]).failing_template_rm();

    let err = rm(&env, &options(Harness::Claude), &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains(&format!("could not remove template {TAG}")),
        "{message}"
    );
    assert!(record_path(&env, "default-claude").exists());
}

#[test]
fn the_cli_runs_template_rm_and_reports_a_refusal() {
    let env = Env::new();

    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["template", "rm", "default", "--harness", "codex"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("profile 'default' has no template for codex"),
        "{stderr}"
    );
}
