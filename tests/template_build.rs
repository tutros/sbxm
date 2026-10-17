//! Milestone 3 slice 4: `sbxm template build` (decisions 201, 204-206, 209-213).

mod common;

use assert_cmd::Command;
use common::{Env, LOW_DISK, low_disk};
use sbxm::backend::{FakeBackend, SandboxBackend};
use sbxm::commands::template_build;
use sbxm::harness::Harness;

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
    template_build::run(&env.config_dir(), options, backend, &mut Vec::new())
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
        &mut out,
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
