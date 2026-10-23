//! Milestone 3 slice 9: `sbxm run`'s contestants and judge follow the profile's template, chosen
//! once per (profile, harness) pair in the preflight (decisions 200, 203, 237).

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::Env;
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::{run, template_build};
use sbxm::harness::Harness;

const CLAUDE_PONG: &str = include_str!("../src/fixtures/claude-stream-json-pong.jsonl");
const TWO_CLAUDES: &str = "[[contestants]]\nharness = \"claude\"\nmodel = \"one\"\n\n\
                           [[contestants]]\nharness = \"claude\"\nmodel = \"two\"\n\n";
const JUDGE: &str = "[[eval.rubric]]\nid = \"correctness\"\nkind = \"pass_fail\"\nweight = 1.0\n\n\
                     [eval.judge]\nharness = \"claude\"\nmodel = \"judge-model\"\n";

const PROFILE: &str = "[network]\n\
     allow = [\"archive.ubuntu.com\"]\n\
     \n\
     [[setup.install]]\n\
     command = \"apt-get install -y ripgrep\"\n\
     \n\
     [template]\n\
     allow = [\"crates.io\"]\n";

fn strict(profile: &str) -> String {
    profile.replace("[template]\n", "[template]\nstrict = true\n")
}

fn backend() -> FakeBackend {
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_version("0.46.0")
        .with_default_exec_output(ExecOutput {
            stdout: CLAUDE_PONG.into(),
            stderr: String::new(),
            exit_code: Some(0),
        })
        // The warm build's own step, as in `tests/template_build.rs`.
        .with_exec_output_matching(
            "cargo metadata",
            ExecOutput {
                stdout: r#"{"packages":[{"name":"x"}],"version":1}"#.into(),
                stderr: String::new(),
                exit_code: Some(0),
            },
        )
}

/// Builds the template of `default` for `harness`, warm from `warm` if given, and returns its tag.
fn build(env: &Env, backend: &FakeBackend, harness: Harness, warm: Option<&Path>) -> String {
    let mut out = Vec::new();
    template_build::run(
        &env.config_dir(),
        &template_build::Options {
            profile: "default".into(),
            harness,
            warm: warm.map(Path::to_path_buf),
            ..Default::default()
        },
        backend,
        &sbxm::task::record::SystemProbe,
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

/// A Cargo repo with `lock` as its `Cargo.lock`.
fn cargo_repo(env: &Env, name: &str, lock: &str) -> PathBuf {
    let repo = env.tmp.path().join(name);
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    fs::write(repo.join("Cargo.lock"), lock).unwrap();
    repo
}

fn seed_line(seed: &Path) -> String {
    format!(
        "seed = {}\n",
        toml::Value::String(seed.to_str().unwrap().to_owned())
    )
}

struct Ran {
    result: anyhow::Result<run::Summary>,
    warn: String,
}

fn go(env: &Env, seed: Option<&Path>, rest: &str, backend: &FakeBackend) -> Ran {
    let path = env.tmp.path().join("run.toml");
    let seed = seed.map(seed_line).unwrap_or_default();
    fs::write(&path, format!("[task]\nprompt = \"p\"\n{seed}\n{rest}")).unwrap();
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = run::run(
        &env.config_dir(),
        &path,
        &Default::default(),
        backend,
        &mut out,
        &mut warn,
    );
    Ran {
        result,
        warn: String::from_utf8(warn).unwrap(),
    }
}

/// Nothing of a run was written: template builds write `.sbxm/templates`, so the run's own two
/// roots are checked.
fn no_run_written(env: &Env) {
    assert!(!env.base_dir().join(".sbxm").join("runs").exists());
    assert!(!env.base_dir().join("runs").exists());
}

fn run_creates(backend: &FakeBackend) -> Vec<sbxm::backend::CreateSpec> {
    backend
        .creates()
        .into_iter()
        .filter(|c| c.name.starts_with("sbxm-run-"))
        .collect()
}

#[test]
fn contestants_and_the_judge_are_created_from_the_profiles_template() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let backend = backend();
    let tag = build(&env, &backend, Harness::Claude, None);

    let ran = go(&env, None, &format!("{TWO_CLAUDES}{JUDGE}"), &backend);

    ran.result.unwrap();
    let creates = run_creates(&backend);
    // Two contestants and the judge.
    assert_eq!(creates.len(), 3, "{creates:?}");
    for create in &creates {
        assert_eq!(create.template.as_deref(), Some(tag.as_str()), "{create:?}");
        let common = fs::read_to_string(create.kits[0].join("spec.yaml")).unwrap();
        assert!(!common.contains("apt-get install -y ripgrep"), "{common}");
        assert!(common.contains("crates.io"), "{common}");
    }
    // The judge's own warnings (same provider, a fake reply) are not about the template.
    assert!(!ran.warn.contains("install steps"), "{}", ran.warn);
}

#[test]
fn a_strict_profile_without_a_template_refuses_the_run_before_any_write() {
    let env = Env::new();
    env.write_profile("default", &strict(PROFILE));
    let backend = backend();

    let ran = go(&env, None, TWO_CLAUDES, &backend);

    let err = format!("{:#}", ran.result.err().unwrap());
    assert!(err.contains("has no template for claude"), "{err}");
    assert!(err.contains("sbxm template build default"), "{err}");
    no_run_written(&env);
    assert!(backend.creates().is_empty());
    assert!(!backend.log().iter().any(|l| l.starts_with("validate ")));
}

#[test]
fn a_warm_template_whose_lock_files_match_the_seed_is_used() {
    let env = Env::new();
    env.write_profile("default", &strict(PROFILE));
    let backend = backend();
    let repo = cargo_repo(&env, "repo", "version = 4\n");
    let tag = build(&env, &backend, Harness::Claude, Some(&repo));
    let seed = cargo_repo(&env, "seed", "version = 4\n");

    let ran = go(&env, Some(&seed), TWO_CLAUDES, &backend);

    ran.result.unwrap();
    let creates = run_creates(&backend);
    assert_eq!(creates.len(), 2, "{creates:?}");
    assert!(
        creates
            .iter()
            .all(|c| c.template.as_deref() == Some(tag.as_str()))
    );
}

#[test]
fn a_seed_whose_lock_files_differ_refuses_a_strict_run_before_any_write() {
    let env = Env::new();
    env.write_profile("default", &strict(PROFILE));
    let backend = backend();
    let repo = cargo_repo(&env, "repo", "version = 4\n");
    build(&env, &backend, Harness::Claude, Some(&repo));
    let seed = cargo_repo(&env, "seed", "version = 4\n# changed\n");

    let ran = go(&env, Some(&seed), TWO_CLAUDES, &backend);

    let err = format!("{:#}", ran.result.err().unwrap());
    assert!(err.contains("is stale"), "{err}");
    assert!(err.contains(&seed.display().to_string()), "{err}");
    no_run_written(&env);
    assert!(run_creates(&backend).is_empty());
}

#[test]
fn a_run_without_a_seed_warns_once_and_installs_with_a_warm_template() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let backend = backend();
    let repo = cargo_repo(&env, "repo", "version = 4\n");
    build(&env, &backend, Harness::Claude, Some(&repo));

    let ran = go(&env, None, TWO_CLAUDES, &backend);

    ran.result.unwrap();
    let creates = run_creates(&backend);
    assert_eq!(creates.len(), 2, "{creates:?}");
    assert!(creates.iter().all(|c| c.template.is_none()));
    let common = fs::read_to_string(creates[0].kits[0].join("spec.yaml")).unwrap();
    assert!(common.contains("apt-get install -y ripgrep"), "{common}");
    let warnings: Vec<&str> = ran.warn.lines().collect();
    assert_eq!(warnings.len(), 1, "{}", ran.warn);
    assert!(
        warnings[0].starts_with("warning: ") && warnings[0].contains("a run without a seed"),
        "{}",
        ran.warn
    );
    // Rebuilding warm can never help a run without lock files (review M3S9-2).
    assert!(warnings[0].contains("set task.seed"), "{}", ran.warn);
    assert!(
        warnings[0].contains("`sbxm template build default`"),
        "{}",
        ran.warn
    );
    assert!(!warnings[0].contains("--warm <repo>"), "{}", ran.warn);
}

#[test]
fn a_strict_run_without_a_seed_and_a_warm_template_names_a_fix_that_works() {
    let env = Env::new();
    env.write_profile("default", &strict(PROFILE));
    let backend = backend();
    let repo = cargo_repo(&env, "repo", "version = 4\n");
    build(&env, &backend, Harness::Claude, Some(&repo));

    let ran = go(&env, None, TWO_CLAUDES, &backend);

    let err = format!("{:#}", ran.result.err().unwrap());
    assert!(
        err.contains("is warm, and a run without a seed has no lock files to check it against"),
        "{err}"
    );
    assert!(err.contains("set task.seed in the run-config"), "{err}");
    assert!(err.contains("`sbxm template build default`"), "{err}");
    assert!(!err.contains("--warm <repo>"), "{err}");
    no_run_written(&env);
}

const CODEX_JUDGE: &str = "[[eval.rubric]]\nid = \"correctness\"\nkind = \"pass_fail\"\nweight = 1.0\n\n\
                           [eval.judge]\nharness = \"codex\"\nmodel = \"judge-model\"\n";

/// A strict profile with a cold claude template for the contestants and a warm codex template
/// only the judge uses; returns the codex tag.
fn judge_only_warm_codex(env: &Env, backend: &FakeBackend) -> String {
    env.write_profile("default", &strict(PROFILE));
    build(env, backend, Harness::Claude, None);
    let repo = cargo_repo(env, "repo", "version = 4\n");
    build(env, backend, Harness::Codex, Some(&repo))
}

fn judge_create(backend: &FakeBackend) -> sbxm::backend::CreateSpec {
    run_creates(backend)
        .into_iter()
        .find(|c| c.name.contains("-judge-"))
        .unwrap_or_else(|| panic!("{:?}", backend.creates()))
}

#[test]
fn a_pair_only_the_judge_uses_gets_no_lock_check_without_a_seed() {
    let env = Env::new();
    let backend = backend();
    let tag = judge_only_warm_codex(&env, &backend);

    let ran = go(&env, None, &format!("{TWO_CLAUDES}{CODEX_JUDGE}"), &backend);

    ran.result.unwrap();
    assert_eq!(
        judge_create(&backend).template.as_deref(),
        Some(tag.as_str())
    );
    assert!(!ran.warn.contains("stale"), "{}", ran.warn);
    assert!(!ran.warn.contains("install steps"), "{}", ran.warn);
}

#[test]
fn a_pair_only_the_judge_uses_gets_no_lock_check_against_the_seed() {
    let env = Env::new();
    let backend = backend();
    let tag = judge_only_warm_codex(&env, &backend);
    let seed = cargo_repo(&env, "seed", "version = 4\n# changed\n");

    let ran = go(
        &env,
        Some(&seed),
        &format!("{TWO_CLAUDES}{CODEX_JUDGE}"),
        &backend,
    );

    ran.result.unwrap();
    assert_eq!(
        judge_create(&backend).template.as_deref(),
        Some(tag.as_str())
    );
    assert!(!ran.warn.contains("stale"), "{}", ran.warn);
}

const CODEX_EXTRA: [&str; 9] = [
    "github.com",
    "api.github.com",
    "codeload.github.com",
    "release-assets.githubusercontent.com",
    "registry.npmjs.org",
    "download.docker.com",
    "archive.ubuntu.com",
    "security.ubuntu.com",
    "ports.ubuntu.com",
];

const ONE_CODEX: &str = "[[contestants]]\nharness = \"codex\"\nmodel = \"c\"\n\n\
                         [[contestants]]\nharness = \"claude\"\nmodel = \"one\"\n\n";

#[test]
fn a_strict_codex_contestant_gets_the_deny_rules_and_is_checked_after_create() {
    let env = Env::new();
    env.write_profile("default", &strict(PROFILE));
    let backend = backend();
    build(&env, &backend, Harness::Claude, None);
    build(&env, &backend, Harness::Codex, None);

    let ran = go(&env, None, ONE_CODEX, &backend);

    let creates = run_creates(&backend);
    let codex_create = creates
        .iter()
        .find(|c| c.agent.contains("codex"))
        .unwrap_or_else(|| panic!("{creates:?}"));
    assert_eq!(codex_create.deny_network, CODEX_EXTRA);
    let claude_create = creates.iter().find(|c| c.agent == "claude").unwrap();
    assert!(claude_create.deny_network.is_empty());
    let log = backend.log();
    let created = log
        .iter()
        .position(|l| *l == format!("create {}", codex_create.name))
        .unwrap();
    assert_eq!(
        log[created + 1],
        format!("network_rules {}", codex_create.name)
    );
    // The fake reports no kit rule, so the check fails closed: the pair errors and its sandbox
    // is removed.
    let err = format!("{:#}", ran.result.err().unwrap());
    assert!(err.contains("ended in error"), "{err}");
    assert!(backend.removes().contains(&codex_create.name));
}

#[test]
fn a_strict_codex_judge_gets_the_deny_rules_is_checked_before_exec_and_removed_when_refused() {
    let env = Env::new();
    env.write_profile("default", &strict(PROFILE));
    let backend = backend();
    build(&env, &backend, Harness::Claude, None);
    let tag = build(&env, &backend, Harness::Codex, None);

    let ran = go(&env, None, &format!("{TWO_CLAUDES}{CODEX_JUDGE}"), &backend);

    let judge = judge_create(&backend);
    assert_eq!(judge.template.as_deref(), Some(tag.as_str()));
    assert_eq!(judge.deny_network, CODEX_EXTRA);
    let log = backend.log();
    let created = log
        .iter()
        .position(|l| *l == format!("create {}", judge.name))
        .unwrap_or_else(|| panic!("{log:?}"));
    assert_eq!(log[created + 1], format!("network_rules {}", judge.name));
    // The fake reports no kit rule, so the check fails closed: the judge never runs and its
    // sandbox is removed.
    assert!(!log.contains(&format!("exec {}", judge.name)), "{log:?}");
    assert_eq!(
        backend
            .removes()
            .iter()
            .filter(|r| **r == judge.name)
            .count(),
        1,
        "{log:?}"
    );
    let shown = match &ran.result {
        Ok(_) => ran.warn.clone(),
        Err(e) => format!("{e:#}\n{}", ran.warn),
    };
    assert!(shown.contains("no network rule"), "{shown}");
}

const PLAIN: &str = "[network]\nallow = [\"archive.ubuntu.com\"]\n";

#[test]
fn a_contestant_under_its_own_install_only_profile_gets_no_tag_next_to_one_with_the_template() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    env.write_profile("plain", PLAIN);
    let backend = backend();
    let tag = build(&env, &backend, Harness::Claude, None);

    let ran = go(
        &env,
        None,
        "[[contestants]]\nharness = \"claude\"\nmodel = \"one\"\n\n\
         [[contestants]]\nharness = \"claude\"\nmodel = \"two\"\nprofile = \"plain\"\n\n",
        &backend,
    );

    ran.result.unwrap();
    let creates = run_creates(&backend);
    assert_eq!(creates.len(), 2, "{creates:?}");
    let templates: Vec<Option<&str>> = creates.iter().map(|c| c.template.as_deref()).collect();
    assert!(templates.contains(&Some(tag.as_str())), "{creates:?}");
    assert!(templates.contains(&None), "{creates:?}");
    let plain = creates.iter().find(|c| c.template.is_none()).unwrap();
    let common = fs::read_to_string(plain.kits[0].join("spec.yaml")).unwrap();
    assert!(!common.contains("crates.io"), "{common}");
}

#[test]
fn an_unreadable_rule_set_leaves_no_codex_contestant_sandbox_after_one_removal() {
    let env = Env::new();
    env.write_profile("default", &strict(PROFILE));
    let backend = backend().failing_network_rules();
    build(&env, &backend, Harness::Claude, None);
    build(&env, &backend, Harness::Codex, None);

    let ran = go(&env, None, ONE_CODEX, &backend);

    let creates = run_creates(&backend);
    let codex = creates
        .iter()
        .find(|c| c.agent.contains("codex"))
        .unwrap_or_else(|| panic!("{creates:?}"));
    let log = backend.log();
    let created = log
        .iter()
        .position(|l| *l == format!("create {}", codex.name))
        .unwrap();
    assert_eq!(log[created + 1], format!("network_rules {}", codex.name));
    assert_eq!(log[created + 2], format!("rm {}", codex.name), "{log:?}");
    assert_eq!(
        log.iter()
            .filter(|l| **l == format!("rm {}", codex.name))
            .count(),
        1,
        "{log:?}"
    );
    assert!(!log.contains(&format!("exec {}", codex.name)), "{log:?}");
    let err = format!("{:#}", ran.result.err().unwrap());
    assert!(err.contains("ended in error"), "{err}");
}
