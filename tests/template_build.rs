//! Milestone 3 slice 4: `sbxm template build` (decisions 201, 204-206, 209-213).

mod common;

use assert_cmd::Command;
use common::{Env, LOW_DISK, low_disk};
use sbxm::backend::FakeBackend;
use sbxm::commands::template_build;
use sbxm::harness::Harness;

const OPTED_IN: &str = "[template]\nallow = []\n";

fn options(harness: Harness) -> template_build::Options {
    template_build::Options {
        profile: "default".into(),
        harness,
        disk: sbxm::disk::Guard::default(),
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
