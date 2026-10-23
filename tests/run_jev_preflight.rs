//! Issue 285 (decision 241): what `sbxm run` refuses for `[eval.jev]` before it writes anything
//! or creates a sandbox: an `sbx` older than 0.46.0, a missing token helper, and a global custom
//! secret with the evaluator's env name and host.

mod common;

use std::path::{Path, PathBuf};

use common::Env;
use sbxm::backend::{CustomSecret, FakeBackend};
use sbxm::commands::run;

fn helper(env: &Env) -> PathBuf {
    let path = env.tmp.path().join("tools").join("jev-token");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "#!/bin/sh\necho canary\n").unwrap();
    path
}

fn config(env: &Env, helper: &Path) -> PathBuf {
    let path = env.tmp.path().join("run.toml");
    let helper = toml::Value::String(helper.to_str().unwrap().to_owned());
    std::fs::write(
        &path,
        format!(
            "[task]\nprompt = \"Do it\"\n\n\
             [[contestants]]\nharness = \"claude\"\nmodel = \"a\"\n\n\
             [[contestants]]\nharness = \"claude\"\nmodel = \"b\"\n\n\
             [[eval.rubric]]\nid = \"correct\"\nkind = \"pass_fail\"\nweight = 1.0\n\n\
             [eval.jev]\ntoken_command = {helper}\n"
        ),
    )
    .unwrap();
    path
}

fn refused(env: &Env, helper: &Path, backend: &FakeBackend) -> String {
    let err = run::run(
        &env.config_dir(),
        &config(env, helper),
        &Default::default(),
        backend,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap_err();
    // Nothing written, nothing created.
    assert!(!env.base_dir().join(".sbxm").exists());
    assert_eq!(std::fs::read_dir(env.base_dir()).unwrap().count(), 0);
    assert!(backend.creates().is_empty(), "{:?}", backend.log());
    assert!(
        !backend.log().iter().any(|l| l.starts_with("create")
            || l.starts_with("secret-set")
            || l.starts_with("validate")),
        "{:?}",
        backend.log()
    );
    format!("{err:#}")
}

fn fine() -> FakeBackend {
    FakeBackend::with_secrets(&["anthropic"]).with_version("0.46.0")
}

#[test]
fn an_sbx_older_than_0_46_0_is_refused() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic"]).with_version("0.45.9");
    let err = refused(&env, &helper(&env), &backend);
    assert!(
        err.contains("0.45.9") && err.contains("0.46.0") && err.contains("eval.jev"),
        "{err}"
    );
}

#[test]
fn a_missing_helper_is_refused() {
    let env = Env::new();
    let missing = env.tmp.path().join("tools").join("nowhere");
    let err = refused(&env, &missing, &fine());
    assert!(
        err.contains("eval.jev.token_command") && err.contains(missing.to_str().unwrap()),
        "{err}"
    );
}

#[test]
fn a_global_secret_with_the_same_env_and_host_is_refused_with_the_removal_command() {
    let env = Env::new();
    let backend = fine().with_custom_secrets(vec![CustomSecret {
        scope: "global".into(),
        targets: vec!["api.typesafe.ai".into()],
        env: "JEV_API_KEY".into(),
        placeholder: "sbx-cs-old".into(),
    }]);
    let err = refused(&env, &helper(&env), &backend);
    assert!(
        err.contains("JEV_API_KEY")
            && err.contains("api.typesafe.ai")
            && err.contains("sbx secret rm --placeholder sbx-cs-old -f"),
        "{err}"
    );
}

#[test]
fn a_scoped_secret_or_another_host_or_env_is_no_conflict() {
    let env = Env::new();
    let secret = |scope: &str, host: &str, name: &str| CustomSecret {
        scope: scope.into(),
        targets: vec![host.into()],
        env: name.into(),
        placeholder: "p".into(),
    };
    let backend = fine().with_custom_secrets(vec![
        secret("sbxm-other", "api.typesafe.ai", "JEV_API_KEY"),
        secret("global", "httpbin.org", "JEV_API_KEY"),
        secret("global", "api.typesafe.ai", "OTHER_KEY"),
    ]);
    let config = sbxm::run::config::RunConfig::load(&config(&env, &helper(&env))).unwrap();
    sbxm::run::preflight::check(&env.config_dir(), &config, &backend).unwrap();
    assert!(backend.log().is_empty(), "{:?}", backend.log());
}

#[test]
fn a_run_without_jev_never_lists_custom_secrets() {
    let env = Env::new();
    // An old sbx and a failing listing are fine when nothing needs them.
    let backend = FakeBackend::with_secrets(&["anthropic"]).failing_custom_secrets();
    let path = env.tmp.path().join("run.toml");
    std::fs::write(
        &path,
        "[task]\nprompt = \"Do it\"\n\n\
         [[contestants]]\nharness = \"claude\"\nmodel = \"a\"\n\n\
         [[contestants]]\nharness = \"claude\"\nmodel = \"b\"\n",
    )
    .unwrap();
    let config = sbxm::run::config::RunConfig::load(&path).unwrap();
    sbxm::run::preflight::check(&env.config_dir(), &config, &backend).unwrap();
}
