//! M2a slice 12b: a contestant may name its own profile; the run's profile
//! applies when it doesn't (decisions 10, 115). Kits and hashes are built per
//! (profile, harness) pair, and the run records which profile each pair used.

mod common;

use std::path::PathBuf;

use common::Env;
use sbxm::backend::{ExecOutput, FakeBackend, SkillsStore};
use sbxm::commands::{run, run_show};
use sbxm::harness::Harness;
use sbxm::run::config::RunConfig;
use sbxm::run::id::RunRoots;
use sbxm::run::kits::{self, RunKits};
use sbxm::run::orchestrate;
use serde_json::Value;

const CLAUDE_PONG: &str = include_str!("../src/fixtures/claude-stream-json-pong.jsonl");
const RUN_ID: &str = "2026-09-30-abc123";

fn contestant(harness: &str, model: &str, profile: Option<&str>) -> String {
    let profile = profile
        .map(|p| format!("profile = \"{p}\"\n"))
        .unwrap_or_default();
    format!("[[contestants]]\nharness = \"{harness}\"\nmodel = \"{model}\"\n{profile}\n")
}

struct Setup {
    env: Env,
    config: RunConfig,
    meta: PathBuf,
}

/// Profiles `default` (env WHO=default) and `strict` (env WHO=strict, skills off).
fn setup(run_table: &str, contestants: &str, extra: &str) -> Setup {
    let env = Env::new();
    env.write_profile("default", "[env]\nWHO = \"default\"\n");
    env.write_profile(
        "strict",
        "[env]\nWHO = \"strict\"\n\n[skills]\nstore = \"off\"\n",
    );
    let path = env.tmp.path().join("run.toml");
    std::fs::write(
        &path,
        format!("[task]\nprompt = \"p\"\n\n{run_table}{contestants}{extra}"),
    )
    .unwrap();
    let config = RunConfig::load(&path).unwrap();
    let meta = env.tmp.path().join("meta").join(RUN_ID);
    std::fs::create_dir_all(&meta).unwrap();
    Setup { env, config, meta }
}

fn build(s: &Setup, backend: &FakeBackend) -> anyhow::Result<RunKits> {
    kits::build(
        &s.env.config_dir(),
        &s.config,
        &s.meta.join("kits"),
        backend,
    )
}

fn common_spec(kits: &RunKits, profile: &str, harness: Harness) -> String {
    std::fs::read_to_string(kits.get_for(profile, harness).unwrap().dirs[0].join("spec.yaml"))
        .unwrap()
}

fn two_profiles() -> String {
    format!(
        "{}{}",
        contestant("claude", "a", None),
        contestant("claude", "b", Some("strict"))
    )
}

// ---- kits per (profile, harness) ----------------------------------------------------

#[test]
fn contestants_with_different_profiles_get_their_own_kits_and_hashes() {
    let s = setup("", &two_profiles(), "");

    let kits = build(&s, &FakeBackend::default()).unwrap();

    // Same harness, two profiles: two kit sets.
    assert_eq!(kits.harnesses.len(), 2);
    let default = kits.get_for("default", Harness::Claude).unwrap();
    let strict = kits.get_for("strict", Harness::Claude).unwrap();
    assert_eq!(
        (default.profile.as_str(), strict.profile.as_str()),
        ("default", "strict")
    );
    assert_ne!(default.config_hash, strict.config_hash);
    assert_ne!(default.dirs, strict.dirs);
    // Each carries its own profile's settings.
    assert!(common_spec(&kits, "default", Harness::Claude).contains("WHO: default"));
    assert!(common_spec(&kits, "strict", Harness::Claude).contains("WHO: strict"));
    assert_eq!(default.skills_store, SkillsStore::ReadOnly);
    assert_eq!(strict.skills_store, SkillsStore::Off);
}

#[test]
fn a_contestant_without_a_profile_uses_the_runs_and_so_does_the_judge() {
    let judge = "[[eval.rubric]]\nid = \"a\"\nkind = \"pass_fail\"\nweight = 1.0\n\n[eval.judge]\nharness = \"codex\"\nmodel = \"j\"\n";
    let s = setup(
        "[run]\nprofile = \"strict\"\n\n",
        &format!(
            "{}{}",
            contestant("claude", "a", None),
            contestant("claude", "b", Some("default"))
        ),
        judge,
    );

    let kits = build(&s, &FakeBackend::default()).unwrap();

    assert_eq!(kits.profile_name, "strict");
    assert!(kits.get_for("strict", Harness::Claude).is_some());
    assert!(kits.get_for("default", Harness::Claude).is_some());
    // The judge has no profile of its own: the run's.
    assert!(kits.get_for("strict", Harness::Codex).is_some());
    assert!(kits.get_for("default", Harness::Codex).is_none());
    // `get` is the run-level profile's entry.
    assert_eq!(kits.get(Harness::Claude).unwrap().profile, "strict");
}

#[test]
fn a_contestant_naming_the_run_profile_shares_its_kit_set() {
    let s = setup(
        "",
        &format!(
            "{}{}",
            contestant("claude", "a", None),
            contestant("claude", "b", Some("default"))
        ),
        "",
    );

    let kits = build(&s, &FakeBackend::default()).unwrap();

    assert_eq!(kits.harnesses.len(), 1);
}

#[test]
fn every_kit_of_every_profile_is_validated_before_anything_is_returned() {
    let s = setup("", &two_profiles(), "");
    let backend = FakeBackend::default();

    let kits = build(&s, &backend).unwrap();

    let expected: Vec<String> = kits
        .harnesses
        .iter()
        .flat_map(|h| h.dirs.iter().map(|d| format!("validate {}", d.display())))
        .collect();
    assert_eq!(backend.log(), expected);
    assert_eq!(expected.len(), 4);
}

#[test]
fn an_invalid_kit_of_a_second_profile_fails_before_any_sandbox() {
    let s = setup("", &two_profiles(), "");
    let backend = FakeBackend::with_invalid_kit("bad");

    let err = build(&s, &backend).unwrap_err().to_string();

    assert!(err.contains("is invalid: bad"), "{err}");
    assert!(backend.creates().is_empty());
}

#[test]
fn an_unknown_contestant_profile_is_an_error_before_anything_is_written() {
    let s = setup(
        "",
        &format!(
            "{}{}",
            contestant("claude", "a", None),
            contestant("claude", "b", Some("ghost"))
        ),
        "",
    );
    let backend = FakeBackend::default();

    let err = format!("{:#}", build(&s, &backend).unwrap_err());

    assert!(err.contains("ghost"), "{err}");
    assert!(!s.meta.join("kits").exists());
    assert!(backend.log().is_empty());
}

// ---- sandboxes get the kits of their own profile ---------------------------------------

#[test]
fn each_pairs_sandbox_is_created_from_its_own_profiles_kits_and_skills_setting() {
    let s = setup("", &two_profiles(), "");
    let kits = build(&s, &FakeBackend::default()).unwrap();
    let workspaces = s.env.tmp.path().join("runs").join(RUN_ID);
    std::fs::create_dir_all(&workspaces).unwrap();
    let roots = RunRoots {
        id: RUN_ID.into(),
        meta: s.meta.clone(),
        workspaces,
    };
    let backend = FakeBackend::default();

    let outcomes = orchestrate::execute(&backend, &s.config, &kits, &roots);

    let creates = backend.creates();
    let of = |i: usize| {
        creates
            .iter()
            .find(|c| c.name == format!("sbxm-run-{RUN_ID}-{i}-0"))
            .unwrap()
    };
    assert_eq!(
        of(0).kits,
        kits.get_for("default", Harness::Claude).unwrap().dirs
    );
    assert_eq!(
        of(1).kits,
        kits.get_for("strict", Harness::Claude).unwrap().dirs
    );
    assert_eq!(
        (of(0).skills, of(1).skills),
        (SkillsStore::ReadOnly, SkillsStore::Off)
    );
    // And each outcome names the profile it ran under.
    assert_eq!(
        outcomes
            .iter()
            .map(|o| o.profile.as_str())
            .collect::<Vec<_>>(),
        ["default", "strict"]
    );
}

// ---- what the run records ---------------------------------------------------------------

fn go(contestants: &str) -> (Env, run::Summary, String) {
    let env = Env::new();
    env.write_profile("default", "[env]\nWHO = \"default\"\n");
    env.write_profile("strict", "[env]\nWHO = \"strict\"\n");
    let backend = FakeBackend::with_secrets(&["anthropic"]).with_default_exec_output(ExecOutput {
        stdout: CLAUDE_PONG.into(),
        stderr: String::new(),
        exit_code: Some(0),
    });
    let path = env.tmp.path().join("run.toml");
    std::fs::write(&path, format!("[task]\nprompt = \"p\"\n\n{contestants}")).unwrap();
    let mut out = Vec::new();
    let summary = run::run(
        &env.config_dir(),
        &path,
        &backend,
        &mut out,
        &mut Vec::new(),
    )
    .unwrap();
    (env, summary, String::from_utf8(out).unwrap())
}

fn read(path: PathBuf) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn run_json_and_result_json_record_each_contestants_profile() {
    let (env, summary, _) = go(&two_profiles());

    let meta = env
        .base_dir()
        .join(".sbxm")
        .join("runs")
        .join(&summary.run_id);
    let record = read(meta.join("run.json"));
    assert_eq!(record["profile"], "default");
    assert_eq!(record["contestants"][0]["profile"], "default");
    assert_eq!(record["contestants"][1]["profile"], "strict");
    let harnesses = record["harnesses"].as_array().unwrap();
    assert_eq!(harnesses.len(), 2);
    let profiles: Vec<&str> = harnesses
        .iter()
        .map(|h| h["profile"].as_str().unwrap())
        .collect();
    assert_eq!(profiles, ["default", "strict"]);
    assert_ne!(harnesses[0]["config_hash"], harnesses[1]["config_hash"]);
    assert_eq!(read(meta.join("0/0/result.json"))["profile"], "default");
    assert_eq!(read(meta.join("1/0/result.json"))["profile"], "strict");
}

#[test]
fn run_show_marks_a_contestant_whose_profile_differs_from_the_runs() {
    let (env, summary, _) = go(&two_profiles());

    let shown = run_show::render(
        &env.config_dir(),
        &summary.run_id,
        &run_show::Options { full_diff: false },
    )
    .unwrap();

    assert!(shown.contains("\ncontestants[0] claude/a\n"), "{shown}");
    assert!(
        shown.contains("\ncontestants[1] claude/b (profile strict)\n"),
        "{shown}"
    );
}

#[test]
fn a_run_with_one_profile_looks_exactly_as_before() {
    let (env, summary, out) = go(&format!(
        "{}{}",
        contestant("claude", "a", None),
        contestant("claude", "b", None)
    ));

    let shown = run_show::render(
        &env.config_dir(),
        &summary.run_id,
        &run_show::Options { full_diff: false },
    )
    .unwrap();
    assert!(!shown.contains("(profile"), "{shown}");
    assert!(!out.contains("profile"), "{out}");
}

#[test]
fn a_contestants_profile_secrets_are_required_before_anything_is_written() {
    let env = Env::new();
    env.write_profile("strict", "[secrets]\nservices = [\"groq\"]\n");
    let backend = FakeBackend::with_secrets(&["anthropic"]);
    let path = env.tmp.path().join("run.toml");
    std::fs::write(
        &path,
        format!("[task]\nprompt = \"p\"\n\n{}", two_profiles()),
    )
    .unwrap();

    let err = run::run(
        &env.config_dir(),
        &path,
        &backend,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap_err()
    .to_string();

    assert!(err.contains("'groq'") && err.contains("strict"), "{err}");
    assert_eq!(std::fs::read_dir(env.base_dir()).unwrap().count(), 0);
}
