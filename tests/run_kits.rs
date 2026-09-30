//! M2a slice 5a: a run's kits are generated, hashed and validated up front,
//! for every harness in use (contestants and judge), before any sandbox is
//! created (decisions 55, 95, 108, 112).

mod common;

use std::path::{Path, PathBuf};

use common::Env;
use sbxm::backend::FakeBackend;
use sbxm::harness::Harness;
use sbxm::run::config::RunConfig;
use sbxm::run::kits::{self, RunKits};

const TASK: &str = "[task]\nprompt = \"Do it\"\n\n";

fn contestant(harness: &str) -> String {
    format!("[[contestants]]\nharness = \"{harness}\"\nmodel = \"m\"\n\n")
}

const JUDGE: &str = "[[eval.rubric]]\nid = \"a\"\nkind = \"pass_fail\"\nweight = 1.0\n\n";

fn judge(harness: &str) -> String {
    format!("{JUDGE}[eval.judge]\nharness = \"{harness}\"\nmodel = \"m\"\n")
}

fn config(env: &Env, body: &str) -> RunConfig {
    let path = env.tmp.path().join("run.toml");
    std::fs::write(&path, body).unwrap();
    RunConfig::load(&path).unwrap()
}

fn kits_root(env: &Env) -> PathBuf {
    env.base_dir()
        .join(".sbxm")
        .join("runs")
        .join("2026-09-30-abc123")
        .join("kits")
}

fn build(env: &Env, body: &str, backend: &FakeBackend) -> anyhow::Result<RunKits> {
    kits::build(
        &env.config_dir(),
        &config(env, body),
        &kits_root(env),
        backend,
    )
}

fn two() -> String {
    format!("{TASK}{}{}", contestant("claude"), contestant("codex"))
}

#[test]
fn one_kit_set_per_harness_in_first_use_order() {
    let env = Env::new();

    let run = build(&env, &two(), &FakeBackend::default()).unwrap();

    let harnesses: Vec<_> = run.harnesses.iter().map(|h| h.harness).collect();
    assert_eq!(harnesses, [Harness::Claude, Harness::Codex]);
}

#[test]
fn two_contestants_of_one_harness_share_one_kit_set() {
    let env = Env::new();
    let body = format!("{TASK}{}{}", contestant("claude"), contestant("claude"));

    let run = build(&env, &body, &FakeBackend::default()).unwrap();

    assert_eq!(run.harnesses.len(), 1);
}

#[test]
fn a_judge_only_harness_gets_kits_and_a_hash_too() {
    let env = Env::new();
    let body = format!(
        "{TASK}{}{}{}",
        contestant("claude"),
        contestant("claude"),
        judge("codex")
    );

    let run = build(&env, &body, &FakeBackend::default()).unwrap();

    let harnesses: Vec<_> = run.harnesses.iter().map(|h| h.harness).collect();
    assert_eq!(harnesses, [Harness::Claude, Harness::Codex]);
    let codex = run.get(Harness::Codex).unwrap();
    assert_eq!(codex.config_hash.len(), 64);
    assert!(codex.dirs.iter().all(|d| d.exists()));
}

#[test]
fn kit_args_are_common_then_the_harness_mixin_under_the_hash_prefix() {
    let env = Env::new();

    let run = build(&env, &two(), &FakeBackend::default()).unwrap();

    for kit in &run.harnesses {
        let prefix = &kit.config_hash[..12];
        let dir = kits_root(&env).join(prefix);
        assert_eq!(
            kit.dirs,
            [
                dir.join("common"),
                dir.join(format!("harness-{}", kit.harness.as_str()))
            ]
        );
    }
}

#[test]
fn every_kit_is_validated_before_anything_is_returned_and_nothing_is_created() {
    let env = Env::new();
    let backend = FakeBackend::default();

    let run = build(&env, &two(), &backend).unwrap();

    let expected: Vec<String> = run
        .harnesses
        .iter()
        .flat_map(|h| h.dirs.iter().map(|d| format!("validate {}", d.display())))
        .collect();
    assert_eq!(backend.log(), expected);
    assert!(backend.creates().is_empty() && backend.execs().is_empty());
}

#[test]
fn an_invalid_kit_fails_the_run_with_zero_creates() {
    let env = Env::new();
    let backend = FakeBackend::with_invalid_kit("bad mixin");

    let err = build(&env, &two(), &backend).unwrap_err().to_string();

    assert!(err.contains("is invalid: bad mixin"), "{err}");
    assert!(err.contains("profile 'default'"), "{err}");
    assert!(backend.creates().is_empty() && backend.execs().is_empty());
    // It stopped at the first bad kit.
    assert_eq!(backend.log().len(), 1, "{:?}", backend.log());
}

#[test]
fn an_invalid_judge_only_kit_also_fails_before_any_contestant_could_be_created() {
    let env = Env::new();
    let backend = FakeBackend::with_invalid_kit("bad judge kit");
    let body = format!(
        "{TASK}{}{}{}",
        contestant("claude"),
        contestant("claude"),
        judge("codex")
    );

    let err = build(&env, &body, &backend).unwrap_err().to_string();

    assert!(err.contains("bad judge kit"), "{err}");
    assert!(backend.creates().is_empty());
}

#[test]
fn the_hash_differs_per_harness_and_is_stable() {
    let env = Env::new();

    let first = build(&env, &two(), &FakeBackend::default()).unwrap();
    let again = build(&env, &two(), &FakeBackend::default()).unwrap();

    let hash = |run: &RunKits, h| run.get(h).unwrap().config_hash.clone();
    assert_ne!(hash(&first, Harness::Claude), hash(&first, Harness::Codex));
    assert_eq!(hash(&first, Harness::Claude), hash(&again, Harness::Claude));
}

#[test]
fn cpu_and_memory_overrides_are_part_of_the_hash_and_the_resources() {
    let env = Env::new();
    let plain = build(&env, &two(), &FakeBackend::default()).unwrap();
    assert_eq!(
        (plain.resources.cpus, plain.resources.memory.as_str()),
        (4, "8g")
    );

    let body = format!(
        "{TASK}[run]\ncpus = 2\nmemory = \"4g\"\n\n{}{}",
        contestant("claude"),
        contestant("codex")
    );
    let sized = build(&env, &body, &FakeBackend::default()).unwrap();

    assert_eq!(
        (sized.resources.cpus, sized.resources.memory.as_str()),
        (2, "4g")
    );
    assert_ne!(
        plain.get(Harness::Claude).unwrap().config_hash,
        sized.get(Harness::Claude).unwrap().config_hash
    );
    // Only one of the two overrides changes the hash too.
    let cpus_only = build(
        &env,
        &format!(
            "{TASK}[run]\ncpus = 2\n\n{}{}",
            contestant("claude"),
            contestant("codex")
        ),
        &FakeBackend::default(),
    )
    .unwrap();
    assert_eq!(cpus_only.resources.memory, "8g");
    assert_ne!(
        cpus_only.get(Harness::Claude).unwrap().config_hash,
        plain.get(Harness::Claude).unwrap().config_hash
    );
}

#[test]
fn the_kits_carry_the_run_profile() {
    let env = Env::new();
    env.write_profile("default", "[network]\nallow = [\"example.org\"]\n");

    let run = build(&env, &two(), &FakeBackend::default()).unwrap();

    let common =
        std::fs::read_to_string(run.get(Harness::Claude).unwrap().dirs[0].join("spec.yaml"))
            .unwrap();
    assert!(common.contains("example.org"), "{common}");
    assert_eq!(run.profile_name, "default");
}

#[test]
fn the_run_profile_option_selects_the_profile() {
    let env = Env::new();
    env.write_profile("strict", "[env]\nWHO = \"strict\"\n");
    let body = format!(
        "{TASK}[run]\nprofile = \"strict\"\n\n{}{}",
        contestant("claude"),
        contestant("codex")
    );

    let run = build(&env, &body, &FakeBackend::default()).unwrap();

    assert_eq!(run.profile_name, "strict");
    let common =
        std::fs::read_to_string(run.get(Harness::Codex).unwrap().dirs[0].join("spec.yaml"))
            .unwrap();
    assert!(common.contains("WHO"), "{common}");
}

#[test]
fn a_contestant_profile_that_differs_is_refused_until_slice_12b() {
    // Per-contestant profiles need kits per (profile, harness); until then
    // they are refused loudly instead of being dropped (decision 11).
    let env = Env::new();
    env.write_profile("strict", "");
    let body = format!(
        "{TASK}[[contestants]]\nharness = \"claude\"\nmodel = \"m\"\nprofile = \"strict\"\n\n{}",
        contestant("codex")
    );
    let backend = FakeBackend::default();

    let err = build(&env, &body, &backend).unwrap_err().to_string();

    assert!(
        err.contains("contestants[0].profile") && err.contains("not supported yet"),
        "{err}"
    );
    assert!(backend.log().is_empty());
    assert!(!kits_root(&env).exists());
}

#[test]
fn a_contestant_profile_equal_to_the_run_profile_is_fine() {
    let env = Env::new();
    let body = format!(
        "{TASK}[[contestants]]\nharness = \"claude\"\nmodel = \"m\"\nprofile = \"default\"\n\n{}",
        contestant("codex")
    );

    build(&env, &body, &FakeBackend::default()).unwrap();
}

#[test]
fn an_unknown_profile_writes_nothing() {
    let env = Env::new();
    let body = format!(
        "{TASK}[run]\nprofile = \"ghost\"\n\n{}{}",
        contestant("claude"),
        contestant("codex")
    );
    let backend = FakeBackend::default();

    let err = format!("{:#}", build(&env, &body, &backend).unwrap_err());

    assert!(err.contains("ghost"), "{err}");
    assert!(!kits_root(&env).exists());
    assert!(backend.log().is_empty());
}

#[test]
fn kits_are_written_only_under_the_given_root() {
    let env = Env::new();

    build(&env, &two(), &FakeBackend::default()).unwrap();

    // Only <base>/.sbxm/runs/<id>/kits exists; nothing in the (mounted) base dir proper.
    let entries: Vec<_> = std::fs::read_dir(env.base_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, [std::ffi::OsString::from(".sbxm")]);
    assert!(Path::new(&kits_root(&env)).is_dir());
}
