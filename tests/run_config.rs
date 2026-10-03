//! M2a slice 4: run-config parsing and validation (decisions 15, 98, 102,
//! 103, 107, 109, 115). Nothing is created; every rejection happens before
//! any filesystem write or backend call.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::Env;
use sbxm::backend::FakeBackend;
use sbxm::harness::Harness;
use sbxm::run::config::{CriterionKind, RunConfig};
use sbxm::run::preflight;

const TASK: &str = "[task]\nprompt = \"Do it\"\n\n";
const CLAUDE: &str =
    "[[contestants]]\nharness = \"claude\"\nmodel = \"claude-haiku-4-5-20251001\"\n\n";
const CODEX: &str = "[[contestants]]\nharness = \"codex\"\nmodel = \"gpt-5.6-luna\"\n\n";
const ANTIGRAVITY: &str =
    "[[contestants]]\nharness = \"antigravity\"\nmodel = \"gemini-3.8-flash-low\"\n\n";

/// Writes `run.toml` next to the config dir and returns its path.
fn write(env: &Env, body: &str) -> PathBuf {
    let path = env.tmp.path().join("run.toml");
    std::fs::write(&path, body).unwrap();
    path
}

fn valid(extra: &str) -> String {
    format!("{TASK}{CLAUDE}{CODEX}{extra}")
}

/// The error of loading `body`, on one line.
fn load_err(body: &str) -> String {
    let env = Env::new();
    let err = RunConfig::load(&write(&env, body)).unwrap_err();
    let text = format!("{err:#}");
    assert!(!text.contains('\n'), "not one line: {text}");
    text
}

fn load_ok(body: &str) -> RunConfig {
    let env = Env::new();
    RunConfig::load(&write(&env, body)).unwrap()
}

// ---- parsing -------------------------------------------------------------

#[test]
fn a_valid_config_parses_with_defaults() {
    let config = load_ok(&valid(""));

    assert_eq!(config.task.prompt, "Do it");
    assert_eq!(config.task.seed, None);
    assert_eq!(config.run.repeat, 1);
    assert_eq!(config.run.timeout, Duration::from_secs(600));
    assert_eq!(config.run.budget_usd, None);
    assert_eq!(config.contestants.len(), 2);
    assert_eq!(config.contestants[0].harness, Harness::Claude);
    assert_eq!(config.contestants[1].model, "gpt-5.6-luna");
    assert!(config.eval.checks.is_empty() && config.eval.rubric.is_empty());
}

#[test]
fn the_full_schema_parses() {
    let config = load_ok(&format!(
        "{TASK}[run]\nprofile = \"default\"\ntimeout = \"90s\"\nbudget_usd = 2.5\ncpus = 2\nmemory = \"4g\"\nrepeat = 3\n\n\
         {CLAUDE}{CODEX}{ANTIGRAVITY}\
         [[eval.checks]]\nid = \"tests-pass\"\ncommand = \"cargo test\"\ntimeout = \"2m\"\n\n\
         [[eval.rubric]]\nid = \"correctness\"\nkind = \"pass_fail\"\nweight = 1.0\n\n\
         [[eval.rubric]]\nid = \"quality\"\nkind = \"scale\"\nlevels = [\"poor\", \"good\"]\nweight = 0.5\nnotes = \"idiomatic\"\n\n\
         [eval.judge]\nharness = \"claude\"\nmodel = \"claude-opus-5-5\"\n"
    ));

    assert_eq!(config.run.timeout, Duration::from_secs(90));
    assert_eq!(config.run.budget_usd, Some(2.5));
    assert_eq!(config.run.cpus, Some(2));
    assert_eq!(config.run.memory.as_deref(), Some("4g"));
    assert_eq!(config.run.repeat, 3);
    assert_eq!(config.contestants.len(), 3);
    assert_eq!(
        config.eval.checks[0].timeout,
        Some(Duration::from_secs(120))
    );
    assert_eq!(config.eval.rubric[1].kind, CriterionKind::Scale);
    assert_eq!(config.eval.rubric[1].levels, ["poor", "good"]);
    assert_eq!(config.eval.judge.as_ref().unwrap().harness, Harness::Claude);
}

#[test]
fn a_relative_seed_is_resolved_against_the_config_file() {
    let env = Env::new();
    std::fs::create_dir_all(env.tmp.path().join("seed-dir")).unwrap();
    let path = write(
        &env,
        &format!("[task]\nprompt = \"p\"\nseed = \"./seed-dir\"\n\n{CLAUDE}{CODEX}"),
    );

    let config = RunConfig::load(&path).unwrap();

    assert_eq!(
        config.task.seed.as_deref(),
        Some(env.tmp.path().join("seed-dir").as_path())
    );
}

// ---- rejections ----------------------------------------------------------

#[test]
fn a_missing_file_says_how_to_create_one() {
    let env = Env::new();
    let path = env.tmp.path().join("nope.toml");

    let err = format!("{:#}", RunConfig::load(&path).unwrap_err());

    assert!(
        err.contains("nope.toml") && err.contains("sbxm run init"),
        "{err}"
    );
}

#[test]
fn unknown_keys_are_errors_at_every_level_and_name_the_file() {
    for (label, body) in [
        ("top level", format!("{}bogus = 1\n", valid(""))),
        (
            "task",
            format!("[task]\nprompt = \"p\"\nbogus = 1\n\n{CLAUDE}{CODEX}"),
        ),
        ("run", format!("{TASK}[run]\nbogus = 1\n\n{CLAUDE}{CODEX}")),
        (
            "contestant",
            format!(
                "{TASK}[[contestants]]\nharness = \"claude\"\nmodel = \"m\"\nbogus = 1\n\n{CODEX}"
            ),
        ),
        ("eval", valid("[eval]\nbogus = 1\n")),
        (
            "check",
            valid("[[eval.checks]]\nid = \"a\"\ncommand = \"c\"\nbogus = 1\n"),
        ),
        (
            "rubric",
            valid("[[eval.rubric]]\nid = \"a\"\nkind = \"pass_fail\"\nweight = 1.0\nbogus = 1\n"),
        ),
        (
            "judge",
            valid("[eval.judge]\nharness = \"claude\"\nmodel = \"m\"\nbogus = 1\n"),
        ),
        ("cosine", valid("[eval.cosine]\nbogus = 1\n")),
    ] {
        let err = load_err(&body);
        assert!(
            err.contains("run.toml") && err.contains("bogus"),
            "{label}: {err}"
        );
    }
}

#[test]
fn cosine_with_no_model_dir_uses_the_default() {
    let config = load_ok(&valid("[eval.cosine]\n"));

    assert_eq!(config.eval.cosine.as_ref().unwrap().model_dir, None);
}

#[test]
fn cosine_with_an_explicit_model_dir_resolves_it_against_the_config_file() {
    let env = Env::new();
    let path = write(
        &env,
        &valid("[eval.cosine]\nmodel_dir = \"./models/mini\"\n"),
    );

    let config = RunConfig::load(&path).unwrap();

    assert_eq!(
        config.eval.cosine.unwrap().model_dir,
        Some(env.tmp.path().join("models").join("mini"))
    );
}

#[test]
fn without_eval_cosine_there_is_none() {
    let config = load_ok(&valid(""));

    assert!(config.eval.cosine.is_none());
}

#[test]
fn contestant_count_must_be_two_to_four() {
    let one = load_err(&format!("{TASK}{CLAUDE}"));
    assert!(one.contains("needs 2 to 4") && one.contains("1"), "{one}");
    let five = load_err(&format!("{TASK}{}", CLAUDE.repeat(5)));
    assert!(
        five.contains("needs 2 to 4") && five.contains("5"),
        "{five}"
    );
    let none = load_err(TASK);
    assert!(none.contains("needs 2 to 4"), "{none}");
    // 2, 3 and 4 are fine.
    for n in 2..=4 {
        load_ok(&format!("{TASK}{}", CLAUDE.repeat(n)));
    }
}

#[test]
fn repeat_must_be_at_least_one() {
    let err = load_err(&format!("{TASK}[run]\nrepeat = 0\n\n{CLAUDE}{CODEX}"));
    assert!(
        err.contains("run.repeat") && err.contains("at least 1"),
        "{err}"
    );
}

#[test]
fn only_claude_codex_and_antigravity_can_be_contestants() {
    let gemini = load_err(&format!(
        "{TASK}{CLAUDE}[[contestants]]\nharness = \"gemini\"\nmodel = \"m\"\n"
    ));
    assert!(
        gemini.contains("contestants[1]")
            && gemini.contains("gemini")
            && gemini.contains("antigravity"),
        "{gemini}"
    );
    let pi = load_err(&format!(
        "{TASK}{CLAUDE}[[contestants]]\nharness = \"pi\"\nmodel = \"m\"\n"
    ));
    assert!(
        pi.contains("contestants[1]") && pi.contains("pi") && pi.contains("deferred"),
        "{pi}"
    );
    let unknown = load_err(&format!(
        "{TASK}{CLAUDE}[[contestants]]\nharness = \"emacs\"\nmodel = \"m\"\n"
    ));
    assert!(
        unknown.contains("emacs") && unknown.contains("claude, codex or antigravity"),
        "{unknown}"
    );
}

#[test]
fn only_claude_codex_and_antigravity_can_judge() {
    for harness in ["gemini", "pi"] {
        let err = load_err(&valid(&format!(
            "[[eval.rubric]]\nid = \"a\"\nkind = \"pass_fail\"\nweight = 1.0\n\n[eval.judge]\nharness = \"{harness}\"\nmodel = \"m\"\n"
        )));
        assert!(err.contains("eval.judge") && err.contains(harness), "{err}");
    }
}

#[test]
fn a_judge_needs_a_rubric() {
    let err = load_err(&valid(
        "[eval.judge]\nharness = \"claude\"\nmodel = \"m\"\n",
    ));
    assert!(
        err.contains("eval.judge") && err.contains("[[eval.rubric]]"),
        "{err}"
    );
}

#[test]
fn empty_prompt_model_and_bad_durations_are_rejected() {
    assert!(
        load_err(&format!("[task]\nprompt = \" \"\n\n{CLAUDE}{CODEX}")).contains("task.prompt")
    );
    assert!(
        load_err(&format!(
            "{TASK}[[contestants]]\nharness = \"claude\"\nmodel = \"\"\n\n{CODEX}"
        ))
        .contains("contestants[0].model")
    );
    for bad in ["10", "0s", "fast", "-5m", "1.5h"] {
        let err = load_err(&format!(
            "{TASK}[run]\ntimeout = \"{bad}\"\n\n{CLAUDE}{CODEX}"
        ));
        assert!(
            err.contains("run.timeout") && err.contains(bad),
            "{bad}: {err}"
        );
    }
}

#[test]
fn budget_and_resources_must_be_positive() {
    for (key, value) in [("budget_usd", "0"), ("budget_usd", "-1.0"), ("cpus", "0")] {
        let err = load_err(&format!("{TASK}[run]\n{key} = {value}\n\n{CLAUDE}{CODEX}"));
        assert!(err.contains(&format!("run.{key}")), "{key}: {err}");
    }
}

#[test]
fn check_ids_and_commands_are_validated() {
    let empty_id = load_err(&valid("[[eval.checks]]\nid = \"\"\ncommand = \"c\"\n"));
    assert!(empty_id.contains("eval.checks[0].id"), "{empty_id}");
    let empty_cmd = load_err(&valid("[[eval.checks]]\nid = \"a\"\ncommand = \" \"\n"));
    assert!(empty_cmd.contains("eval.checks[0].command"), "{empty_cmd}");
    let dup = load_err(&valid(
        "[[eval.checks]]\nid = \"a\"\ncommand = \"c\"\n\n[[eval.checks]]\nid = \"a\"\ncommand = \"d\"\n",
    ));
    assert!(dup.contains("duplicate") && dup.contains("\"a\""), "{dup}");
    let bad_timeout = load_err(&valid(
        "[[eval.checks]]\nid = \"a\"\ncommand = \"c\"\ntimeout = \"soon\"\n",
    ));
    assert!(
        bad_timeout.contains("eval.checks[0].timeout"),
        "{bad_timeout}"
    );
}

#[test]
fn rubric_criteria_are_validated() {
    let criterion = |body: &str| valid(&format!("[[eval.rubric]]\n{body}"));
    let cases = [
        (
            "id = \"\"\nkind = \"pass_fail\"\nweight = 1.0\n",
            "eval.rubric[0].id",
        ),
        (
            "id = \"a\"\nkind = \"pass_fail\"\nweight = 1.0\n\n[[eval.rubric]]\nid = \"a\"\nkind = \"pass_fail\"\nweight = 1.0\n",
            "duplicate",
        ),
        (
            "id = \"a\"\nkind = \"pass_fail\"\nweight = 0.0\n",
            "eval.rubric[0].weight",
        ),
        (
            "id = \"a\"\nkind = \"pass_fail\"\nweight = -1.0\n",
            "eval.rubric[0].weight",
        ),
        (
            "id = \"a\"\nkind = \"pass_fail\"\nweight = nan\n",
            "eval.rubric[0].weight",
        ),
        (
            "id = \"a\"\nkind = \"pass_fail\"\nweight = inf\n",
            "eval.rubric[0].weight",
        ),
        (
            "id = \"a\"\nkind = \"scale\"\nweight = 1.0\n",
            "at least 2 distinct levels",
        ),
        (
            "id = \"a\"\nkind = \"scale\"\nlevels = [\"only\"]\nweight = 1.0\n",
            "at least 2 distinct levels",
        ),
        (
            "id = \"a\"\nkind = \"scale\"\nlevels = [\"same\", \"same\"]\nweight = 1.0\n",
            "at least 2 distinct levels",
        ),
        (
            "id = \"a\"\nkind = \"pass_fail\"\nlevels = [\"x\", \"y\"]\nweight = 1.0\n",
            "levels",
        ),
        ("id = \"a\"\nkind = \"vibes\"\nweight = 1.0\n", "vibes"),
    ];
    for (body, expected) in cases {
        let err = load_err(&criterion(body));
        assert!(err.contains(expected), "{body}: {err}");
    }
}

// ---- preflight: seed, secrets, warnings ---------------------------------

fn preflight_of(
    env: &Env,
    body: &str,
    backend: &FakeBackend,
) -> anyhow::Result<preflight::Preflight> {
    let config = RunConfig::load(&write(env, body))?;
    preflight::check(&env.config_dir(), &config, backend)
}

fn assert_nothing_created(env: &Env, backend: &FakeBackend) {
    assert!(!env.base_dir().join(".sbxm").exists());
    assert_eq!(std::fs::read_dir(env.base_dir()).unwrap().count(), 0);
    assert!(backend.creates().is_empty() && backend.execs().is_empty());
    assert!(backend.log().is_empty(), "{:?}", backend.log());
}

#[test]
fn a_valid_run_with_stored_secrets_passes() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);

    let result = preflight_of(&env, &valid(""), &backend).unwrap();

    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_nothing_created(&env, &backend);
}

#[test]
fn a_missing_contestant_provider_secret_is_refused() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic"]);

    let err = preflight_of(&env, &valid(""), &backend)
        .unwrap_err()
        .to_string();

    assert_eq!(
        err,
        "secret 'openai' (needed by contestants[1], codex) is not stored in sbx; add it with \
         `sbx secret set openai` or import it with `sbx setup`"
    );
    assert_nothing_created(&env, &backend);
}

#[test]
fn antigravity_needs_the_google_secret() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);

    let err = preflight_of(&env, &format!("{TASK}{CLAUDE}{ANTIGRAVITY}"), &backend)
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("'google'") && err.contains("antigravity"),
        "{err}"
    );
}

#[test]
fn the_judges_provider_is_checked_even_if_no_contestant_uses_it() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let body = format!(
        "{TASK}{CLAUDE}{CODEX}[[eval.rubric]]\nid = \"a\"\nkind = \"pass_fail\"\nweight = 1.0\n\n\
         [eval.judge]\nharness = \"antigravity\"\nmodel = \"gemini-3.8-flash-low\"\n"
    );

    let err = preflight_of(&env, &body, &backend).unwrap_err().to_string();

    assert!(
        err.contains("'google'") && err.contains("eval.judge"),
        "{err}"
    );
    assert_nothing_created(&env, &backend);
}

#[test]
fn a_profile_named_service_is_checked_too() {
    let env = Env::new();
    env.write_profile("default", "[secrets]\nservices = [\"github\"]\n");
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);

    let err = preflight_of(&env, &valid(""), &backend)
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("'github'") && err.contains("secrets.services"),
        "{err}"
    );
    assert!(err.contains("sbx secret set github"), "{err}");
    assert_nothing_created(&env, &backend);
}

#[test]
fn a_contestant_profile_is_loaded_and_its_services_are_checked() {
    let env = Env::new();
    env.write_profile("strict", "[secrets]\nservices = [\"groq\"]\n");
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let body = format!(
        "{TASK}[[contestants]]\nharness = \"claude\"\nmodel = \"m\"\nprofile = \"strict\"\n\n{CODEX}"
    );

    let err = preflight_of(&env, &body, &backend).unwrap_err().to_string();

    assert!(err.contains("'groq'") && err.contains("strict"), "{err}");
}

#[test]
fn an_unknown_profile_is_refused_before_anything_else() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);

    let err = format!(
        "{:#}",
        preflight_of(
            &env,
            &format!("{TASK}[run]\nprofile = \"ghost\"\n\n{CLAUDE}{CODEX}"),
            &backend
        )
        .unwrap_err()
    );

    assert!(err.contains("ghost"), "{err}");
    assert_nothing_created(&env, &backend);
}

#[test]
fn secrets_are_deduplicated_across_contestants_judge_and_profiles() {
    let env = Env::new();
    env.write_profile("default", "[secrets]\nservices = [\"anthropic\"]\n");
    // Two Claude contestants, a Claude judge and a profile naming anthropic:
    // one secret, one stored entry is enough.
    let backend = FakeBackend::with_secrets(&["anthropic"]);
    let body = format!(
        "{TASK}{CLAUDE}{CLAUDE}[[eval.rubric]]\nid = \"a\"\nkind = \"pass_fail\"\nweight = 1.0\n\n\
         [eval.judge]\nharness = \"claude\"\nmodel = \"m\"\n"
    );

    preflight_of(&env, &body, &backend).unwrap();
}

#[test]
fn a_seed_that_is_missing_or_holds_links_is_refused() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let with_seed = |seed: &std::path::Path| {
        format!(
            "[task]\nprompt = \"p\"\nseed = {}\n\n{CLAUDE}{CODEX}",
            toml::Value::String(seed.to_str().unwrap().to_owned())
        )
    };

    let missing = preflight_of(&env, &with_seed(&env.tmp.path().join("nope")), &backend)
        .unwrap_err()
        .to_string();
    assert!(missing.contains("is not a directory"), "{missing}");

    let seed = env.seed();
    let target = env.tmp.path().join("elsewhere-target");
    std::fs::create_dir_all(&target).unwrap();
    common::dir_link(&seed.join("link"), &target);
    assert!(
        seed.join("link").symlink_metadata().is_ok(),
        "link not created"
    );
    let linked = preflight_of(&env, &with_seed(&seed), &backend)
        .unwrap_err()
        .to_string();
    assert!(linked.contains("symlink or junction"), "{linked}");
    assert_nothing_created(&env, &backend);
}

#[test]
fn a_seed_containing_the_base_dir_is_refused() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let body = format!(
        "[task]\nprompt = \"p\"\nseed = {}\n\n{CLAUDE}{CODEX}",
        toml::Value::String(env.tmp.path().to_str().unwrap().to_owned())
    );

    let err = preflight_of(&env, &body, &backend).unwrap_err().to_string();

    assert!(err.contains("contains the base dir"), "{err}");
}

const COSINE_MODEL_FILES: [&str; 5] = [
    "model.onnx",
    "tokenizer.json",
    "config.json",
    "special_tokens_map.json",
    "tokenizer_config.json",
];

#[test]
fn a_cosine_model_missing_a_file_is_refused_naming_it() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let dir = env.tmp.path().join("models");
    std::fs::create_dir_all(&dir).unwrap();
    for name in COSINE_MODEL_FILES.iter().skip(1) {
        std::fs::write(dir.join(name), "{}").unwrap();
    }
    let body = format!(
        "{TASK}{CLAUDE}{CODEX}[eval.cosine]\nmodel_dir = {}\n",
        toml::Value::String(dir.to_str().unwrap().to_owned())
    );

    let err = preflight_of(&env, &body, &backend).unwrap_err().to_string();

    assert!(err.contains("model.onnx"), "{err}");
    assert!(err.contains(dir.to_str().unwrap()), "{err}");
    assert!(
        err.contains("https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx/resolve/main/"),
        "{err}"
    );
    assert_nothing_created(&env, &backend);
}

#[test]
fn a_cosine_model_with_every_file_present_passes() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let dir = env.tmp.path().join("models");
    std::fs::create_dir_all(&dir).unwrap();
    for name in COSINE_MODEL_FILES {
        std::fs::write(dir.join(name), "{}").unwrap();
    }
    let body = format!(
        "{TASK}{CLAUDE}{CODEX}[eval.cosine]\nmodel_dir = {}\n",
        toml::Value::String(dir.to_str().unwrap().to_owned())
    );

    let result = preflight_of(&env, &body, &backend).unwrap();

    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_nothing_created(&env, &backend);
}

#[test]
fn a_cosine_model_with_no_model_dir_is_checked_under_the_config_dir() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);

    let err = preflight_of(&env, &valid("[eval.cosine]\n"), &backend)
        .unwrap_err()
        .to_string();

    let default_dir = env.config_dir().join("models").join("all-minilm-l6-v2");
    assert!(err.contains(default_dir.to_str().unwrap()), "{err}");
    assert_nothing_created(&env, &backend);
}

#[test]
fn budget_with_a_harness_that_has_no_budget_flag_warns_loudly() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);

    let result = preflight_of(
        &env,
        &format!("{TASK}[run]\nbudget_usd = 2.0\n\n{CLAUDE}{CODEX}"),
        &backend,
    )
    .unwrap();

    assert_eq!(
        result.warnings,
        [
            "run.budget_usd is set, but contestants[1] (codex) has no budget flag: only the timeout \
          limits its cost"
        ]
    );
}

#[test]
fn no_budget_no_warning() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);

    let result = preflight_of(&env, &valid(""), &backend).unwrap();

    assert!(result.warnings.is_empty());
}

#[test]
fn unsupported_profile_settings_warn_for_each_contestant_and_the_judge() {
    let env = Env::new();
    env.write_profile(
        "default",
        "[skills]\nstore = \"readonly\"\n[harness.claude]\nmanaged_settings = \"managed.json\"\n",
    );
    std::fs::write(
        env.profiles_dir().join("default").join("managed.json"),
        "{}",
    )
    .unwrap();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai", "google"]);
    let body = format!(
        "{TASK}{CLAUDE}{CODEX}[[eval.rubric]]\nid = \"a\"\nkind = \"pass_fail\"\nweight = 1.0\n\n\
         [eval.judge]\nharness = \"antigravity\"\nmodel = \"gemini-3.8-flash-low\"\n"
    );

    let warnings = preflight_of(&env, &body, &backend).unwrap().warnings;

    let joined = warnings.join("\n");
    // Codex: managed settings don't apply. Antigravity (judge): managed settings and skills store.
    assert!(
        joined.contains("harness.claude.managed_settings is set, but codex"),
        "{joined}"
    );
    assert!(
        joined.contains("harness.claude.managed_settings is set, but antigravity"),
        "{joined}"
    );
    assert!(
        joined.contains("skills.store is \"readonly\", but antigravity"),
        "{joined}"
    );
    assert!(!joined.contains("but claude"), "{joined}");
}
