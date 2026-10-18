//! Milestone 3 slice 4: `sbxm template build` (decisions 201, 204-206, 209-213).

mod common;

use assert_cmd::Command;
use common::{Env, LOW_DISK, low_disk};
use sbxm::backend::{FakeBackend, SandboxBackend};
use sbxm::commands::template_build;
use sbxm::harness::Harness;
use sbxm::task::record::SystemProbe;

const OPTED_IN: &str = "[template]\nallow = []\n";

fn options(harness: Harness) -> template_build::Options {
    template_build::Options {
        profile: "default".into(),
        harness,
        disk: sbxm::disk::Guard::default(),
        now: None,
    }
}

fn build(
    env: &Env,
    options: &template_build::Options,
    backend: &FakeBackend,
) -> anyhow::Result<()> {
    template_build::run(
        &env.config_dir(),
        options,
        backend,
        &SystemProbe,
        &mut Vec::new(),
        &mut Vec::new(),
    )
}

/// The refusal, after checking that nothing was written and no backend call was made.
fn refused(env: &Env, options: &template_build::Options, backend: &FakeBackend) -> String {
    let err = build(env, options, backend).unwrap_err();
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(!env.base_dir().join(".sbxm").exists());
    format!("{err:#}")
}

fn sbx_046() -> FakeBackend {
    FakeBackend::default().with_version("0.46.0")
}

#[test]
fn an_unsupported_harness_is_refused() {
    let env = Env::new();
    env.write_profile("default", OPTED_IN);

    for harness in [Harness::Antigravity, Harness::Gemini] {
        let message = refused(&env, &options(harness), &sbx_046());

        assert!(
            message.contains(&format!(
                "templates are not supported for the {} harness yet; build one for claude, codex or pi",
                harness.as_str()
            )),
            "{message}"
        );
    }
}

#[test]
fn a_profile_without_template_table_is_refused() {
    let env = Env::new();

    let message = refused(&env, &options(Harness::Claude), &sbx_046());

    let path = env.profiles_dir().join("default").join("profile.toml");
    assert!(
        message.contains(&format!(
            "profile 'default' has no [template] table; add one with its runtime allow list to {} \
             to opt in",
            path.display()
        )),
        "{message}"
    );
}

#[test]
fn an_sbx_older_than_the_template_minimum_is_refused() {
    let env = Env::new();
    env.write_profile("default", OPTED_IN);
    let backend = FakeBackend::default().with_version("0.45.9");

    let message = refused(&env, &options(Harness::Claude), &backend);

    assert!(
        message
            .contains("sbx 0.45.9 is older than 0.46.0, the version templates need; upgrade sbx"),
        "{message}"
    );
}

#[test]
fn too_little_free_disk_is_refused_before_any_write() {
    let env = Env::new();
    env.write_profile("default", OPTED_IN);
    let options = template_build::Options {
        disk: low_disk(&env),
        ..options(Harness::Claude)
    };

    let message = refused(&env, &options, &sbx_046());

    assert!(message.contains(LOW_DISK), "{message}");
}

#[test]
fn the_cli_runs_template_build_and_reports_a_refusal() {
    let env = Env::new();
    env.write_profile("default", OPTED_IN);

    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["template", "build", "default", "--harness", "gemini"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("templates are not supported for the gemini harness yet"),
        "{stderr}"
    );
}

// Step 4.2: a cold build, end to end.

/// 2023-11-14T22:13:20Z.
const NOW: u64 = 1_700_000_000;

const BUILD_PROFILE: &str = "[network]\n\
     allow = [\"archive.ubuntu.com\"]\n\
     \n\
     [[setup.install]]\n\
     command = \"apt-get install -y ripgrep\"\n\
     \n\
     [template]\n\
     allow = []\n";

fn at_now(harness: Harness) -> template_build::Options {
    template_build::Options {
        now: Some(NOW),
        ..options(harness)
    }
}

fn expected_tag(env: &Env, harness: Harness) -> String {
    let config = sbxm::config::GlobalConfig::load(&env.config_dir()).unwrap();
    let profile = sbxm::config::Profile::load(config.profiles_dir(), "default").unwrap();
    let hash = sbxm::kit::template_hash(&profile, harness, false);
    format!(
        "sbxm-default-{}:{}-20231114t2213",
        harness.as_str(),
        &hash[..12]
    )
}

fn build_dir(env: &Env) -> std::path::PathBuf {
    env.base_dir().join(".sbxm").join("templates").join("build")
}

fn record(env: &Env) -> serde_json::Value {
    let path = env
        .base_dir()
        .join(".sbxm")
        .join("templates")
        .join("default-claude.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn a_cold_build_creates_stops_saves_and_removes_the_build_sandbox() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let backend = sbx_046();

    build(&env, &at_now(Harness::Claude), &backend).unwrap();

    let tag = expected_tag(&env, Harness::Claude);
    let kits = build_dir(&env).join("default-claude.kits");
    assert_eq!(
        backend.log(),
        vec![
            format!("validate {}", kits.join("common").display()),
            format!("validate {}", kits.join("harness-claude").display()),
            "create sbxm-template-default-claude".to_owned(),
            "stop sbxm-template-default-claude".to_owned(),
            format!("template-save sbxm-template-default-claude {tag}"),
            "rm sbxm-template-default-claude".to_owned(),
        ]
    );
}

#[test]
fn the_build_sandbox_gets_the_install_steps_and_the_build_allow_list() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let hook_seen = seen.clone();
    let backend = sbx_046().with_create_hook(move |spec| {
        let common = std::fs::read_to_string(spec.kits[0].join("spec.yaml")).unwrap();
        *hook_seen.lock().unwrap() = common;
        assert!(spec.workspace.is_dir(), "{}", spec.workspace.display());
    });

    build(&env, &at_now(Harness::Claude), &backend).unwrap();

    let create = &backend.creates()[0];
    let kits = build_dir(&env).join("default-claude.kits");
    assert_eq!(create.template, None);
    assert_eq!(create.agent, "claude");
    assert_eq!(create.workspace, build_dir(&env).join("default-claude"));
    assert_eq!(
        create.kits,
        vec![kits.join("common"), kits.join("harness-claude")]
    );
    let common = seen.lock().unwrap().clone();
    assert!(common.contains("apt-get install -y ripgrep"), "{common}");
    assert!(common.contains("archive.ubuntu.com"), "{common}");
}

#[test]
fn a_cold_build_writes_its_record() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let backend = sbx_046();

    build(&env, &at_now(Harness::Claude), &backend).unwrap();

    let tag = expected_tag(&env, Harness::Claude);
    let size = backend
        .template_ls()
        .unwrap()
        .into_iter()
        .find(|t| t.reference() == tag)
        .unwrap()
        .size;
    let config = sbxm::config::GlobalConfig::load(&env.config_dir()).unwrap();
    let profile = sbxm::config::Profile::load(config.profiles_dir(), "default").unwrap();
    assert_eq!(
        record(&env),
        serde_json::json!({
            "tag": tag,
            "template_hash": sbxm::kit::template_hash(&profile, Harness::Claude, false),
            "warm": false,
            "lock_files": [],
            "lock_hash": null,
            "sbx_version": "0.46.0",
            "size_bytes": size,
            "created_at": "2023-11-14T22:13:20Z",
            "previous": [],
        })
    );
}

#[test]
fn a_cold_build_leaves_no_workspace_or_kits_behind() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);

    build(&env, &at_now(Harness::Claude), &sbx_046()).unwrap();

    let left: Vec<_> = std::fs::read_dir(build_dir(&env))
        .map(|d| d.map(|e| e.unwrap().file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn a_cold_build_prints_its_tag() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let mut out = Vec::new();

    template_build::run(
        &env.config_dir(),
        &at_now(Harness::Codex),
        &sbx_046(),
        &SystemProbe,
        &mut out,
        &mut Vec::new(),
    )
    .unwrap();

    assert_eq!(
        String::from_utf8(out).unwrap(),
        format!("{}\n", expected_tag(&env, Harness::Codex))
    );
}

#[test]
fn a_generated_tag_passes_the_removal_guard() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);

    for harness in [Harness::Claude, Harness::Codex, Harness::Pi] {
        sbxm::backend::check_template_tag(&expected_tag(&env, harness)).unwrap();
    }
}

#[test]
fn ignore_disk_reaches_sandbox_creation_on_a_low_disk() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let mut disk = low_disk(&env);
    disk.ignore = true;
    let options = template_build::Options {
        disk,
        ..at_now(Harness::Claude)
    };
    let backend = sbx_046();

    build(&env, &options, &backend).unwrap();

    assert_eq!(backend.creates().len(), 1);
}

#[test]
fn a_tag_that_already_exists_is_refused_before_any_write() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let tag = expected_tag(&env, Harness::Claude);
    let (name, version) = tag.split_once(':').unwrap();
    let backend = sbx_046().with_templates(vec![sbxm::backend::TemplateImage {
        id: "x".into(),
        repository: format!("docker.io/library/{name}"),
        tag: version.into(),
        flavor: "claude-code-docker".into(),
        created_at: "2023-11-14T22:13:00Z".into(),
        size: 1,
    }]);

    let message = refused(&env, &at_now(Harness::Claude), &backend);

    assert!(
        message.contains(&format!(
            "template {tag} already exists; wait a minute and build again"
        )),
        "{message}"
    );
}

// Step 4.3: a failed build leaves nothing behind.

/// The build's leftovers after a failed build: build folder entries, the record, and the
/// templates sbxm saved.
fn assert_nothing_left(env: &Env, backend: &FakeBackend) {
    let left: Vec<_> = std::fs::read_dir(build_dir(env))
        .map(|d| d.map(|e| e.unwrap().file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
    let record = env
        .base_dir()
        .join(".sbxm")
        .join("templates")
        .join("default-claude.json");
    assert!(!record.exists());
    assert!(backend.template_ls().unwrap().is_empty());
}

#[test]
fn a_failed_create_removes_the_build_sandbox_and_folders() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let backend = FakeBackend::failing_create().with_version("0.46.0");

    let err = build(&env, &at_now(Harness::Claude), &backend).unwrap_err();

    assert!(
        format!("{err:#}").contains("fake create failure"),
        "{err:#}"
    );
    assert_eq!(
        &backend.log()[2..],
        [
            "create sbxm-template-default-claude",
            "rm sbxm-template-default-claude"
        ]
    );
    assert_nothing_left(&env, &backend);
}

#[test]
fn a_failed_stop_removes_the_build_sandbox_and_folders() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let backend = FakeBackend::failing_stop().with_version("0.46.0");

    let err = build(&env, &at_now(Harness::Claude), &backend).unwrap_err();

    assert!(format!("{err:#}").contains("fake stop failure"), "{err:#}");
    assert_eq!(
        backend.log().last().unwrap(),
        "rm sbxm-template-default-claude"
    );
    assert_nothing_left(&env, &backend);
}

#[test]
fn a_failed_save_removes_the_build_sandbox_and_folders() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let backend = FakeBackend::failing_template_save().with_version("0.46.0");

    let err = build(&env, &at_now(Harness::Claude), &backend).unwrap_err();

    assert!(format!("{err:#}").contains("fake save failure"), "{err:#}");
    assert_eq!(
        backend.log().last().unwrap(),
        "rm sbxm-template-default-claude"
    );
    assert_nothing_left(&env, &backend);
}

#[test]
fn an_invalid_kit_removes_the_kits_without_a_sandbox_call() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let backend = FakeBackend::with_invalid_kit("bad kit").with_version("0.46.0");

    let err = build(&env, &at_now(Harness::Claude), &backend).unwrap_err();

    assert!(format!("{err:#}").contains("bad kit"), "{err:#}");
    assert!(
        backend.log().iter().all(|l| l.starts_with("validate ")),
        "{:?}",
        backend.log()
    );
    assert_nothing_left(&env, &backend);
}

#[test]
fn a_failure_after_the_save_removes_the_saved_tag_and_names_the_leftover_sandbox() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let backend = FakeBackend::failing_remove().with_version("0.46.0");

    let err = build(&env, &at_now(Harness::Claude), &backend).unwrap_err();

    let tag = expected_tag(&env, Harness::Claude);
    let message = format!("{err:#}");
    assert!(message.contains("fake remove failure"), "{message}");
    assert!(
        message.contains(
            "could not remove build sandbox sbxm-template-default-claude; remove it with \
             `sbx rm sbxm-template-default-claude`"
        ),
        "{message}"
    );
    assert_eq!(
        &backend.log()[5..],
        [
            "rm sbxm-template-default-claude".to_owned(),
            "rm sbxm-template-default-claude".to_owned(),
            format!("template-rm {tag}"),
        ]
    );
    assert_nothing_left(&env, &backend);
}

// Step 4.4: the previous tag goes unless a sandbox or task still records it (decision 205a).

/// One minute after [`NOW`], so a second build gets a fresh tag.
const LATER: u64 = NOW + 60;

fn build_at(env: &Env, now: u64, backend: &FakeBackend) -> String {
    let mut out = Vec::new();
    let options = template_build::Options {
        now: Some(now),
        ..options(Harness::Claude)
    };
    template_build::run(
        &env.config_dir(),
        &options,
        backend,
        &SystemProbe,
        &mut out,
        &mut Vec::new(),
    )
    .unwrap();
    String::from_utf8(out).unwrap()
}

/// Records `tag` as the template of project `demo`'s Claude sandbox in its `state.json`.
fn sandbox_uses(env: &Env, tag: &str) {
    let dir = env.base_dir().join(".sbxm").join("demo");
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
    state.save(&dir).unwrap();
}

/// Writes a task whose worker records `tag` as its template; returns the task's id.
fn task_uses(env: &Env, tag: &str) -> String {
    use sbxm::task::record::{self, Agent, Kind, NewTask, Process, Record};
    let mut task = Record::new(
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
    task.worker = Some(Agent {
        harness: "claude".into(),
        model: None,
        sandbox: "sbxm-task-7-claude".into(),
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

fn tag_at(env: &Env, now: u64) -> String {
    let tag = expected_tag(env, Harness::Claude);
    let stamp = if now == NOW {
        "20231114t2213"
    } else {
        "20231114t2214"
    };
    format!("{}{stamp}", &tag[..tag.len() - 13])
}

fn template_tags(backend: &FakeBackend) -> Vec<String> {
    backend
        .template_ls()
        .unwrap()
        .iter()
        .map(|t| t.reference())
        .collect()
}

#[test]
fn a_rebuild_removes_the_unused_previous_tag() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let backend = sbx_046();
    build_at(&env, NOW, &backend);

    let out = build_at(&env, LATER, &backend);

    let (old, new) = (tag_at(&env, NOW), tag_at(&env, LATER));
    assert_eq!(backend.log().last().unwrap(), &format!("template-rm {old}"));
    assert_eq!(template_tags(&backend), [new.as_str()]);
    assert_eq!(record(&env)["previous"], serde_json::json!([]));
    assert_eq!(out, format!("{new}\nremoved previous template {old}\n"));
}

#[test]
fn a_rebuild_with_an_unchanged_hash_keeps_a_previous_tag_a_sandbox_uses() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let backend = sbx_046();
    build_at(&env, NOW, &backend);
    let old = tag_at(&env, NOW);
    sandbox_uses(&env, &old);

    let out = build_at(&env, LATER, &backend);

    let new = tag_at(&env, LATER);
    assert!(!backend.log().iter().any(|l| l.starts_with("template-rm")));
    assert_eq!(template_tags(&backend), [old.clone(), new.clone()]);
    assert_eq!(record(&env)["tag"], serde_json::json!(new));
    assert_eq!(record(&env)["previous"], serde_json::json!([old]));
    assert_eq!(
        out,
        format!(
            "{new}\nkept previous template {old}: still used by sandbox sbxm-demo-claude; a later \
             build removes it once nothing uses it\n"
        )
    );
}

#[test]
fn a_rebuild_keeps_a_previous_tag_a_task_uses() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let backend = sbx_046();
    build_at(&env, NOW, &backend);
    let old = tag_at(&env, NOW);
    let id = task_uses(&env, &old);

    let out = build_at(&env, LATER, &backend);

    assert!(!backend.log().iter().any(|l| l.starts_with("template-rm")));
    assert_eq!(record(&env)["previous"], serde_json::json!([old]));
    assert!(out.contains(&format!("still used by task {id};")), "{out}");
}

#[test]
fn a_kept_tag_goes_with_a_later_build_once_unused() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let backend = sbx_046();
    build_at(&env, NOW, &backend);
    let old = tag_at(&env, NOW);
    sandbox_uses(&env, &old);
    build_at(&env, LATER, &backend);
    std::fs::remove_dir_all(env.base_dir().join(".sbxm").join("demo")).unwrap();

    build_at(&env, LATER + 60, &backend);

    let log = backend.log();
    assert!(log.contains(&format!("template-rm {old}")), "{log:?}");
    assert!(
        log.contains(&format!("template-rm {}", tag_at(&env, LATER))),
        "{log:?}"
    );
    assert_eq!(record(&env)["previous"], serde_json::json!([]));
    assert_eq!(template_tags(&backend).len(), 1);
}

#[test]
fn a_previous_tag_already_gone_is_skipped() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let backend = sbx_046();
    build_at(&env, NOW, &backend);
    backend.template_rm(&tag_at(&env, NOW)).unwrap();
    let removals = backend.log().len();

    let out = build_at(&env, LATER, &backend);

    assert!(
        !backend.log()[removals..]
            .iter()
            .any(|l| l.starts_with("template-rm")),
        "{:?}",
        backend.log()
    );
    assert_eq!(out, format!("{}\n", tag_at(&env, LATER)));
    assert_eq!(record(&env)["previous"], serde_json::json!([]));
}

#[test]
fn a_previous_tag_that_cannot_be_removed_stays_in_the_record() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let first = sbx_046();
    build_at(&env, NOW, &first);
    let old = tag_at(&env, NOW);
    let backend = sbx_046()
        .with_templates(first.template_ls().unwrap())
        .failing_template_rm();

    let out = build_at(&env, LATER, &backend);

    assert!(
        out.contains(&format!(
            "could not remove previous template {old} (fake template rm failure); a later build \
             retries, or remove it with `sbx template rm {old}`"
        )),
        "{out}"
    );
    assert_eq!(record(&env)["previous"], serde_json::json!([old]));
}

#[test]
fn a_previous_tag_is_kept_when_a_task_record_cannot_be_read() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let backend = sbx_046();
    build_at(&env, NOW, &backend);
    let old = tag_at(&env, NOW);
    let task = env.base_dir().join(".sbxm").join("tasks").join("broken");
    std::fs::create_dir_all(&task).unwrap();
    std::fs::write(task.join("task.json"), "not json").unwrap();

    let out = build_at(&env, LATER, &backend);

    assert!(
        out.contains(&format!(
            "kept previous template {old}: cannot tell whether it is in use ("
        )),
        "{out}"
    );
    assert!(!backend.log().iter().any(|l| l.starts_with("template-rm")));
    assert_eq!(record(&env)["previous"], serde_json::json!([old]));
}

// Decision 228: like `new`, a missing base dir is refused and kit warnings are shown.

#[test]
fn a_missing_base_dir_is_refused_before_any_write() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    std::fs::remove_dir_all(env.base_dir()).unwrap();

    let message = refused(&env, &at_now(Harness::Claude), &sbx_046());

    assert!(
        message.contains(&format!(
            "base dir {} does not exist; create it or change base_dir in {}",
            env.base_dir().display(),
            env.config_dir().join("config.toml").display()
        )),
        "{message}"
    );
}

#[test]
fn kit_warnings_are_shown() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let backend = sbx_046().with_kit_warning("unknown field x");
    let mut warn = Vec::new();

    template_build::run(
        &env.config_dir(),
        &at_now(Harness::Claude),
        &backend,
        &SystemProbe,
        &mut Vec::new(),
        &mut warn,
    )
    .unwrap();

    let kits = build_dir(&env).join("default-claude.kits");
    assert_eq!(
        String::from_utf8(warn).unwrap(),
        format!(
            "warning: kit {}: \"unknown field x\"\nwarning: kit {}: \"unknown field x\"\n",
            kits.join("common").display(),
            kits.join("harness-claude").display()
        )
    );
}

// Step 4.5: the build lock (decision 212).

/// A process table where every pid started at this time, or none exists.
struct Probe(Option<u64>);

impl sbxm::task::record::ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        self.0
    }
}

fn lock_path(env: &Env) -> std::path::PathBuf {
    env.base_dir()
        .join(".sbxm")
        .join("templates")
        .join("default-claude.build.json")
}

/// A lock left by process 4242, started at [`NOW`].
fn write_lock(env: &Env) {
    std::fs::create_dir_all(lock_path(env).parent().unwrap()).unwrap();
    std::fs::write(
        lock_path(env),
        r#"{"pid": 4242, "started_at": "2023-11-14T22:13:20Z"}"#,
    )
    .unwrap();
}

/// Builds with `probe`, returning the result and the warnings.
fn build_with(env: &Env, probe: &Probe, backend: &FakeBackend) -> (anyhow::Result<()>, String) {
    let mut warn = Vec::new();
    let result = template_build::run(
        &env.config_dir(),
        &at_now(Harness::Claude),
        backend,
        probe,
        &mut Vec::new(),
        &mut warn,
    );
    (result, String::from_utf8(warn).unwrap())
}

#[test]
fn a_build_is_refused_while_the_locking_process_is_alive() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    write_lock(&env);
    let backend = sbx_046();

    let (result, _) = build_with(&env, &Probe(Some(NOW)), &backend);

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains(
            "a template build of default-claude is already running (process 4242, started \
             2023-11-14T22:13:20Z); wait for it to finish"
        ),
        "{message}"
    );
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(!build_dir(&env).exists());
    assert!(lock_path(&env).exists());
}

#[test]
fn an_interrupted_builds_leftovers_are_removed_and_the_build_goes_on() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    write_lock(&env);
    for dir in ["default-claude", "default-claude.kits"] {
        let dir = build_dir(&env).join(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old"), "x").unwrap();
    }
    let backend = FakeBackend::with_sandboxes(vec![sbxm::backend::SandboxInfo {
        name: "sbxm-template-default-claude".into(),
        agent: "claude".into(),
        status: "stopped".into(),
    }])
    .with_version("0.46.0");

    let (result, warn) = build_with(&env, &Probe(None), &backend);

    result.unwrap();
    assert_eq!(backend.log()[0], "rm sbxm-template-default-claude");
    assert_eq!(
        warn,
        "removed the leftovers of an interrupted template build (process 4242, started \
         2023-11-14T22:13:20Z)\n"
    );
    assert!(!build_dir(&env).join("default-claude").exists());
    assert!(!lock_path(&env).exists());
}

#[test]
fn a_lock_whose_pid_was_reused_counts_as_interrupted() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    write_lock(&env);
    let backend = sbx_046();

    let (result, _) = build_with(&env, &Probe(Some(NOW + 1)), &backend);

    result.unwrap();
    assert!(
        backend.log()[0].starts_with("validate "),
        "{:?}",
        backend.log()
    );
}

#[test]
fn the_lock_names_this_process_during_the_build() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let path = lock_path(&env);
    let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let hook_seen = seen.clone();
    let backend = sbx_046().with_create_hook(move |_| {
        *hook_seen.lock().unwrap() = std::fs::read_to_string(&path).unwrap();
    });

    let (result, _) = build_with(&env, &Probe(Some(NOW)), &backend);

    result.unwrap();
    let lock: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()).unwrap();
    assert_eq!(
        lock,
        serde_json::json!({"pid": std::process::id(), "started_at": "2023-11-14T22:13:20Z"})
    );
    assert!(!lock_path(&env).exists());
}

#[test]
fn the_lock_is_removed_after_a_failed_build() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let backend = FakeBackend::failing_create().with_version("0.46.0");

    let (result, _) = build_with(&env, &Probe(Some(NOW)), &backend);

    assert!(result.is_err());
    assert!(!lock_path(&env).exists());
}

#[test]
fn an_unreadable_lock_is_refused_with_how_to_clear_it() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    std::fs::create_dir_all(lock_path(&env).parent().unwrap()).unwrap();
    std::fs::write(lock_path(&env), "not json").unwrap();
    let backend = sbx_046();

    let (result, _) = build_with(&env, &Probe(None), &backend);

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains(&format!(
            "cannot read the build lock {}; if no template build of default-claude is running, \
             delete it",
            lock_path(&env).display()
        )),
        "{message}"
    );
    assert!(backend.log().is_empty(), "{:?}", backend.log());
}

// Slice 4 review: M-1 (taking over a stale lock), S-1 (the tag re-check), S-3 (leftover folders).

/// Process 4242 is gone; the first time it is looked up, another build takes the lock over and
/// writes its own (process 5555, alive).
struct RacingProbe {
    path: std::path::PathBuf,
}

impl sbxm::task::record::ProcessProbe for RacingProbe {
    fn start_time(&self, pid: u32) -> Option<u64> {
        if pid == 4242 {
            std::fs::write(
                &self.path,
                r#"{"pid": 5555, "started_at": "2023-11-14T22:13:20Z"}"#,
            )
            .unwrap();
            return None;
        }
        Some(NOW)
    }
}

#[test]
fn a_lock_another_build_took_over_meanwhile_is_left_alone() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    write_lock(&env);
    let backend = sbx_046();
    let probe = RacingProbe {
        path: lock_path(&env),
    };

    let result = template_build::run(
        &env.config_dir(),
        &at_now(Harness::Claude),
        &backend,
        &probe,
        &mut Vec::new(),
        &mut Vec::new(),
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("already running (process 5555, started 2023-11-14T22:13:20Z)"),
        "{message}"
    );
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(
        std::fs::read_to_string(lock_path(&env))
            .unwrap()
            .contains("5555")
    );
}

#[test]
fn a_build_is_refused_while_another_takes_over_an_interrupted_one() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    write_lock(&env);
    let takeover = lock_path(&env).with_file_name("default-claude.build.takeover");
    std::fs::write(&takeover, "").unwrap();
    let backend = sbx_046();

    let (result, _) = build_with(&env, &Probe(None), &backend);

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains(&format!(
            "another template build of default-claude is clearing an interrupted build ({}); \
             try again, or delete it if no template build is running",
            takeover.display()
        )),
        "{message}"
    );
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(lock_path(&env).exists());
}

/// Saves `tag` into the backend the first time any process is looked up, as a build that
/// finished while this one waited for the lock would have.
struct SavingProbe<'a> {
    backend: &'a FakeBackend,
    tag: String,
    done: std::sync::atomic::AtomicBool,
}

impl sbxm::task::record::ProcessProbe for SavingProbe<'_> {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        if !self.done.swap(true, std::sync::atomic::Ordering::SeqCst) {
            self.backend.template_save("other", &self.tag).unwrap();
        }
        Some(NOW)
    }
}

#[test]
fn a_tag_saved_while_waiting_for_the_lock_is_refused() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let backend = sbx_046();
    let tag = expected_tag(&env, Harness::Claude);
    let probe = SavingProbe {
        backend: &backend,
        tag: tag.clone(),
        done: Default::default(),
    };

    let result = template_build::run(
        &env.config_dir(),
        &at_now(Harness::Claude),
        &backend,
        &probe,
        &mut Vec::new(),
        &mut Vec::new(),
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains(&format!(
            "template {tag} already exists; wait a minute and build again"
        )),
        "{message}"
    );
    assert!(backend.creates().is_empty());
    assert!(!lock_path(&env).exists());
}

#[test]
fn an_interrupted_builds_folders_are_gone_before_the_new_build_creates() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    write_lock(&env);
    let olds: Vec<_> = ["default-claude", "default-claude.kits"]
        .iter()
        .map(|dir| build_dir(&env).join(dir).join("old"))
        .collect();
    for old in &olds {
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(old, "x").unwrap();
    }
    let seen = olds.clone();
    let backend = sbx_046().with_create_hook(move |_| {
        for old in &seen {
            assert!(!old.exists(), "{}", old.display());
        }
    });

    let (result, _) = build_with(&env, &Probe(None), &backend);

    result.unwrap();
}

#[test]
fn an_unreadable_record_is_refused_with_how_to_fix_it() {
    let env = Env::new();
    env.write_profile("default", BUILD_PROFILE);
    let path = env
        .base_dir()
        .join(".sbxm")
        .join("templates")
        .join("default-claude.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{\"tag\": ").unwrap();
    let backend = sbx_046();

    let (result, _) = build_with(&env, &Probe(Some(NOW)), &backend);

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains(&format!(
            "template record {} is not valid; delete it, then remove the sbxm- templates it named \
             with `sbx template ls` and `sbx template rm`",
            path.display()
        )),
        "{message}"
    );
    assert!(backend.creates().is_empty());
}
