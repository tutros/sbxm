//! Milestone 3 slice 6: `new` and `open --rebuild` create from a template (decisions 200, 203, 208,
//! 213, 214).

mod common;

use common::Env;
use sbxm::backend::{FakeBackend, NetworkRule, SandboxInfo};
use sbxm::commands::{new, open, template_build};
use sbxm::config;
use sbxm::harness::Harness;

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

fn sbx_046() -> FakeBackend {
    FakeBackend::default().with_version("0.46.0")
}

/// Builds the template of `default` for claude and returns its tag.
fn build_template(env: &Env, backend: &FakeBackend) -> String {
    let mut out = Vec::new();
    template_build::run(
        &env.config_dir(),
        &template_build::Options {
            profile: "default".into(),
            harness: Harness::Claude,
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

fn new_demo(env: &Env, backend: &FakeBackend) -> (anyhow::Result<()>, String) {
    let mut warn = Vec::new();
    let result = new::run(
        &env.config_dir(),
        "demo",
        &new::Options::default(),
        backend,
        &mut warn,
    );
    (result, String::from_utf8(warn).unwrap())
}

fn state_template(env: &Env) -> serde_json::Value {
    let path = env.base_dir().join(".sbxm").join("demo").join("state.json");
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    state["sandboxes"]["claude"]["template"].clone()
}

/// The `common` kit of the sandbox `new` created last.
fn last_common_kit(backend: &FakeBackend) -> String {
    let create = backend.creates().pop().unwrap();
    std::fs::read_to_string(create.kits[0].join("spec.yaml")).unwrap()
}

#[test]
fn a_matching_template_is_used_with_the_runtime_kit() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let backend = sbx_046();
    let tag = build_template(&env, &backend);

    let (result, warn) = new_demo(&env, &backend);

    result.unwrap();
    assert_eq!(warn, "");
    let create = backend.creates().pop().unwrap();
    assert_eq!(create.template.as_deref(), Some(tag.as_str()));
    let common = last_common_kit(&backend);
    assert!(!common.contains("apt-get install -y ripgrep"), "{common}");
    assert!(common.contains("crates.io"), "{common}");
    assert!(!common.contains("archive.ubuntu.com"), "{common}");
    assert_eq!(state_template(&env), tag.as_str());
}

#[test]
fn a_profile_without_template_never_asks_sbx_about_templates() {
    let env = Env::new();
    let backend = FakeBackend::default();

    let (result, _) = new_demo(&env, &backend);

    result.unwrap();
    assert_eq!(backend.creates()[0].template, None);
    assert!(
        !backend.log().iter().any(|l| l.starts_with("template")),
        "{:?}",
        backend.log()
    );
    assert_eq!(state_template(&env), serde_json::Value::Null);
}

/// What `new` says when the template can't be used: a strict profile is refused before any
/// write; a non-strict one warns once and installs.
fn check_unusable(env: &Env, backend: &FakeBackend, reason: &str, fix: &str) {
    let profile =
        std::fs::read_to_string(env.profiles_dir().join("default").join("profile.toml")).unwrap();
    let creates = backend.creates().len();

    env.write_profile("default", &strict(&profile));
    let (result, _) = new_demo(env, backend);
    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains(&format!("{reason}; {fix}")),
        "{message}\nshould contain: {reason}; {fix}"
    );
    assert_eq!(backend.creates().len(), creates, "strict creates nothing");
    assert!(!env.base_dir().join("demo").exists());
    assert!(!env.base_dir().join(".sbxm").join("demo").exists());

    env.write_profile("default", &profile);
    let (result, warn) = new_demo(env, backend);
    result.unwrap();
    assert_eq!(
        warn,
        format!(
            "warning: {reason}; creating sbxm-demo-claude with the profile's install steps \
             instead ({fix})\n"
        )
    );
    assert_eq!(backend.creates().pop().unwrap().template, None);
    assert!(last_common_kit(backend).contains("apt-get install -y ripgrep"));
    assert_eq!(state_template(env), serde_json::Value::Null);
}

#[test]
fn a_missing_template_is_refused_when_strict_and_installs_otherwise() {
    let env = Env::new();
    env.write_profile("default", PROFILE);

    check_unusable(
        &env,
        &sbx_046(),
        "profile 'default' has no template for claude",
        "build one with `sbxm template build default`",
    );
}

#[test]
fn a_template_whose_image_is_gone_counts_as_missing() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let built = sbx_046();
    build_template(&env, &built);

    check_unusable(
        &env,
        &sbx_046(),
        "profile 'default' has no template for claude",
        "build one with `sbxm template build default`",
    );
}

#[test]
fn a_template_with_a_changed_build_part_is_stale() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let backend = sbx_046();
    build_template(&env, &backend);
    env.write_profile("default", &PROFILE.replace("ripgrep", "ripgrep jq"));

    check_unusable(
        &env,
        &backend,
        "the template of profile 'default' for claude is stale: the profile's install steps, \
         build allow list or harness kit changed since it was built",
        "rebuild it with `sbxm template build default`",
    );
}

#[test]
fn a_runtime_change_does_not_make_the_template_stale() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let backend = sbx_046();
    let tag = build_template(&env, &backend);
    env.write_profile(
        "default",
        &format!(
            "{}\n[env]\nFOO = \"bar\"\n",
            PROFILE.replace("crates.io", "docs.rs")
        ),
    );

    let (result, _) = new_demo(&env, &backend);

    result.unwrap();
    assert_eq!(
        backend.creates().pop().unwrap().template.as_deref(),
        Some(tag.as_str())
    );
}

#[test]
fn an_sbx_older_than_the_template_minimum_counts_as_missing() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let backend = sbx_046();
    build_template(&env, &backend);
    let old = backend.with_version("0.45.0");

    check_unusable(
        &env,
        &old,
        "sbx 0.45.0 is older than 0.46.0, the version templates need",
        "upgrade sbx",
    );
}

#[test]
fn a_project_override_adding_build_inputs_cannot_use_the_template() {
    for (key, toml) in [
        (
            "setup.install",
            "[[setup.install]]\ncommand = \"apt-get install -y jq\"\n",
        ),
        ("network.allow", "[network]\nallow = [\"example.com\"]\n"),
        ("network.deny", "[network]\ndeny = [\"example.com\"]\n"),
    ] {
        let env = Env::new();
        env.write_profile("default", PROFILE);
        let backend = sbx_046();
        build_template(&env, &backend);
        let project = env.base_dir().join(".sbxm").join("demo");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join("sandbox.toml");
        std::fs::write(&path, toml).unwrap();
        let profile =
            std::fs::read_to_string(env.profiles_dir().join("default").join("profile.toml"))
                .unwrap();

        env.write_profile("default", &strict(&profile));
        let (result, _) = new_demo(&env, &backend);
        let message = format!("{:#}", result.unwrap_err());
        assert!(
            message.contains(&format!(
                "project config {} adds {key}, which the template of profile 'default' can't \
                 hold; move it to the profile and rebuild the template, or remove it",
                path.display()
            )),
            "{message}"
        );
        assert!(backend.creates().len() == 1, "only the build sandbox");

        env.write_profile("default", &profile);
        let (result, warn) = new_demo(&env, &backend);
        result.unwrap();
        assert!(
            warn.contains("creating sbxm-demo-claude with the profile's install steps instead"),
            "{warn}"
        );
        assert_eq!(backend.creates().pop().unwrap().template, None);
    }
}

/// Creates `project` and returns the config hash its state records.
fn hash_of(env: &Env, backend: &FakeBackend, project: &str) -> String {
    new::run(
        &env.config_dir(),
        project,
        &new::Options::default(),
        backend,
        &mut Vec::new(),
    )
    .unwrap();
    let path = env
        .base_dir()
        .join(".sbxm")
        .join(project)
        .join("state.json");
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    state["sandboxes"]["claude"]["config_hash"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn a_template_sandbox_hashes_the_runtime_allow_list_and_an_install_one_does_not() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let without = FakeBackend::default();
    let install = hash_of(&env, &without, "one");
    let backend = sbx_046();
    build_template(&env, &backend);
    let from_template = hash_of(&env, &backend, "two");
    assert_ne!(install, from_template, "the two variants differ");
    let kits = |project: &str| {
        std::fs::read_dir(env.base_dir().join(".sbxm").join(project).join("kits"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>()
    };
    assert_ne!(kits("one"), kits("two"), "each variant has its own kit dir");

    env.write_profile("default", &PROFILE.replace("crates.io", "docs.rs"));

    assert_eq!(hash_of(&env, &without, "three"), install);
    assert_ne!(hash_of(&env, &backend, "four"), from_template);
}

#[test]
fn strict_is_part_of_the_hash_of_both_variants() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let backend = sbx_046();
    build_template(&env, &backend);
    let lax = hash_of(&env, &backend, "one");

    env.write_profile("default", &strict(PROFILE));

    assert_ne!(hash_of(&env, &backend, "two"), lax);
    let install = || {
        let resources = config::GlobalConfig::load(&env.config_dir())
            .unwrap()
            .resources;
        let profile = config::Profile::load(&env.profiles_dir(), "default").unwrap();
        config::config_hash("default", &profile, &resources, Harness::Claude)
    };
    let strict_install = install();
    env.write_profile("default", PROFILE);
    assert_ne!(install(), strict_install);
}

fn open_demo(env: &Env, backend: &FakeBackend, rebuild: bool) -> (anyhow::Result<()>, String) {
    let mut warn = Vec::new();
    let result = open::run(
        &env.config_dir(),
        "demo",
        &open::Options {
            rebuild,
            ..Default::default()
        },
        backend,
        &mut warn,
    );
    (result, String::from_utf8(warn).unwrap())
}

fn running() -> Vec<SandboxInfo> {
    vec![SandboxInfo {
        name: "sbxm-demo-claude".into(),
        agent: "claude".into(),
        status: "running".into(),
    }]
}

#[test]
fn open_checks_a_template_sandbox_against_the_template_variant() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let backend = sbx_046();
    build_template(&env, &backend);
    new_demo(&env, &backend).0.unwrap();
    let backend = backend.and_sandboxes(running());

    let (result, warn) = open_demo(&env, &backend, false);

    result.unwrap();
    assert_eq!(warn, "");
    assert_eq!(backend.log().last().unwrap(), "attach sbxm-demo-claude");
}

#[test]
fn open_hints_at_a_rebuild_once_a_template_is_ready_and_rebuild_uses_it() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let backend = sbx_046();
    new_demo(&env, &backend).0.unwrap();
    let backend = backend.and_sandboxes(running());
    let (result, warn) = open_demo(&env, &backend, false);
    result.unwrap();
    assert_eq!(warn, "", "no template yet, no hint");
    let tag = build_template(&env, &backend);

    let (result, warn) = open_demo(&env, &backend, false);

    result.unwrap();
    assert_eq!(
        warn,
        "hint: the template of profile 'default' for claude is ready; `sbxm open demo --harness claude --rebuild` \
         recreates sbxm-demo-claude from it (its session history is lost; the workspace is kept)\n"
    );
    let (result, _) = open_demo(&env, &backend, true);
    result.unwrap();
    assert_eq!(
        backend.creates().pop().unwrap().template.as_deref(),
        Some(tag.as_str())
    );
    assert_eq!(state_template(&env), tag.as_str());
}

#[test]
fn list_checks_a_template_sandbox_against_the_template_variant() {
    use sbxm::commands::list::{self, ConfigStatus};
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let backend = sbx_046();
    build_template(&env, &backend);
    new_demo(&env, &backend).0.unwrap();
    let backend = backend.and_sandboxes(running());
    let config = || list::entries(&env.config_dir(), &backend).unwrap()[0].config;

    assert_eq!(config(), Some(ConfigStatus::Current));
    env.write_profile("default", &PROFILE.replace("crates.io", "docs.rs"));
    assert_eq!(config(), Some(ConfigStatus::Changed));
}

fn new_demo_with(
    env: &Env,
    backend: &FakeBackend,
    harness: Harness,
) -> (anyhow::Result<()>, String) {
    let mut warn = Vec::new();
    let result = new::run(
        &env.config_dir(),
        "demo",
        &new::Options {
            harness,
            ..Default::default()
        },
        backend,
        &mut warn,
    );
    (result, String::from_utf8(warn).unwrap())
}

#[test]
fn a_strict_profile_refuses_a_harness_templates_do_not_support() {
    // Review PR 269 round 2 M-1, decision 240.
    for harness in [Harness::Antigravity, Harness::Gemini] {
        let env = Env::new();
        env.write_profile("default", &strict(PROFILE));
        let backend = sbx_046();

        let (result, _) = new_demo_with(&env, &backend, harness);

        let message = format!("{:#}", result.unwrap_err());
        assert!(
            message.contains(&format!(
                "templates are not supported for the {} harness yet; use --harness claude, codex or pi",
                harness.as_str()
            )),
            "{message}"
        );
        assert!(!message.contains("template build"), "{message}");
        assert!(backend.creates().is_empty());
        assert!(!env.base_dir().join("demo").exists());
    }
}

#[test]
fn a_non_strict_profile_warns_and_installs_for_a_harness_templates_do_not_support() {
    for harness in [Harness::Antigravity, Harness::Gemini] {
        let env = Env::new();
        env.write_profile("default", PROFILE);
        let backend = sbx_046();

        let (result, warn) = new_demo_with(&env, &backend, harness);

        result.unwrap();
        assert!(
            warn.contains(&format!(
                "warning: templates are not supported for the {} harness yet; creating \
                 sbxm-demo-{} with the profile's install steps instead",
                harness.as_str(),
                harness.as_str()
            )),
            "{warn}"
        );
        assert!(!warn.contains("template build"), "{warn}");
        assert_eq!(backend.creates().pop().unwrap().template, None);
    }
}

/// `config show demo --kits`, and the hash `state.json` records for it.
fn show_demo(env: &Env) -> (String, String) {
    let output = sbxm::commands::config_show::render(
        &env.config_dir(),
        Some("demo"),
        &sbxm::commands::config_show::Options {
            kits: true,
            ..Default::default()
        },
    )
    .unwrap();
    let path = env.base_dir().join(".sbxm").join("demo").join("state.json");
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let recorded = state["sandboxes"]["claude"]["config_hash"]
        .as_str()
        .unwrap()
        .to_owned();
    (output, recorded)
}

#[test]
fn config_show_explains_a_template_sandbox_with_the_template_variant() {
    // Review PR 269 round 2 M-2.
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let backend = sbx_046();
    build_template(&env, &backend);
    new_demo(&env, &backend).0.unwrap();

    let (output, recorded) = show_demo(&env);

    assert!(
        output.contains(&format!("# config hash: {recorded}\n")),
        "{output}"
    );
    let (input, kits) = output.split_once("\n# kit: ").unwrap();
    assert!(
        input.contains("[template]\nstrict = false\nallow = [\"crates.io\"]"),
        "{input}"
    );
    assert!(!kits.contains("apt-get install -y ripgrep"), "{kits}");
    assert!(kits.contains("crates.io"), "{kits}");
    assert!(!kits.contains("archive.ubuntu.com"), "{kits}");
}

#[test]
fn config_show_explains_an_install_sandbox_with_the_install_variant() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let backend = sbx_046();
    new_demo(&env, &backend).0.unwrap();

    let (output, recorded) = show_demo(&env);

    assert!(
        output.contains(&format!("# config hash: {recorded}\n")),
        "{output}"
    );
    let (_, kits) = output.split_once("\n# kit: ").unwrap();
    assert!(kits.contains("apt-get install -y ripgrep"), "{kits}");
    assert!(kits.contains("archive.ubuntu.com"), "{kits}");
}

#[test]
fn a_template_sandbox_whose_runtime_allow_list_changed_is_drift() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let backend = sbx_046();
    build_template(&env, &backend);
    new_demo(&env, &backend).0.unwrap();
    let backend = backend.and_sandboxes(running());
    env.write_profile("default", &PROFILE.replace("crates.io", "docs.rs"));

    let (result, _) = open_demo(&env, &backend, false);

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("the config of sbxm-demo-claude (profile 'default') changed"),
        "{message}"
    );
}

/// Builds a warm template of `default` from a Rust repo whose `Cargo.lock` holds `lock`.
fn build_warm_template(env: &Env, lock: &str) -> FakeBackend {
    let repo = env.tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
    std::fs::write(repo.join("Cargo.lock"), lock).unwrap();
    let backend = sbx_046().with_exec_output_matching(
        "cargo metadata",
        sbxm::backend::ExecOutput {
            stdout: r#"{"packages":[{"name":"app"}],"version":1}"#.into(),
            stderr: String::new(),
            exit_code: Some(0),
        },
    );
    template_build::run(
        &env.config_dir(),
        &template_build::Options {
            profile: "default".into(),
            harness: Harness::Claude,
            warm: Some(repo),
            ..Default::default()
        },
        &backend,
        &sbxm::task::record::SystemProbe,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap();
    backend
}

#[test]
fn a_warm_template_is_used_while_the_workspace_lock_files_match() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let backend = build_warm_template(&env, "version = 4\n");
    let workspace = env.base_dir().join("demo");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("Cargo.lock"), "version = 4\n").unwrap();

    let (result, warn) = new_demo(&env, &backend);

    result.unwrap();
    assert_eq!(warn, "");
    assert!(backend.creates().pop().unwrap().template.is_some());
}

#[test]
fn a_warm_template_is_stale_when_the_workspace_lock_files_differ() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let backend = build_warm_template(&env, "version = 4\n");
    let workspace = env.base_dir().join("demo");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("Cargo.lock"), "version = 3\n").unwrap();

    let (result, warn) = new_demo(&env, &backend);

    result.unwrap();
    assert_eq!(
        warn,
        format!(
            "warning: the template of profile 'default' for claude is stale: the lock files of {} \
             (Cargo.lock) differ from the ones it was warmed with; creating sbxm-demo-claude with \
             the profile's install steps instead (rebuild it with `sbxm template build default \
             --warm <repo>`)\n",
            workspace.display()
        )
    );
    assert_eq!(backend.creates().pop().unwrap().template, None);
}

// Decision 221: a strict `codex` sandbox denies the `codex` kit's extra hosts and is checked after
// its creation.

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

/// The rules `sbx` reports for a `codex` sandbox: the kit's allow rule (with `extra` added to it)
/// and one deny rule per host in `denied`.
fn codex_rules(extra: &[&str], denied: &[&str]) -> Vec<NetworkRule> {
    let mut kit: Vec<String> = ["api.openai.com:443", "chatgpt.com:443"]
        .iter()
        .chain(CODEX_EXTRA.iter())
        .chain(extra)
        .map(|h| {
            if h.contains(':') {
                h.to_string()
            } else {
                format!("{h}:443")
            }
        })
        .collect();
    kit.dedup();
    let mut rules = vec![NetworkRule {
        name: "kit:sbxm-demo-codex".into(),
        scope: SCOPE.into(),
        allow: true,
        resources: kit,
    }];
    rules.extend(denied.iter().map(|h| NetworkRule {
        name: "some-uuid".into(),
        scope: SCOPE.into(),
        allow: false,
        resources: vec![h.to_string()],
    }));
    rules
}

const SCOPE: &str = "sandbox:sbxm-demo-codex";

fn build_codex_template(env: &Env, backend: &FakeBackend) {
    template_build::run(
        &env.config_dir(),
        &template_build::Options {
            profile: "default".into(),
            harness: Harness::Codex,
            ..Default::default()
        },
        backend,
        &sbxm::task::record::SystemProbe,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap();
}

fn new_codex(env: &Env, backend: &FakeBackend) -> anyhow::Result<()> {
    new::run(
        &env.config_dir(),
        "demo",
        &new::Options {
            harness: Harness::Codex,
            ..Default::default()
        },
        backend,
        &mut Vec::new(),
    )
}

fn codex_state(env: &Env) -> serde_json::Value {
    let path = env.base_dir().join(".sbxm").join("demo").join("state.json");
    match std::fs::read_to_string(path) {
        Ok(text) => {
            serde_json::from_str::<serde_json::Value>(&text).unwrap()["sandboxes"]["codex"].clone()
        }
        Err(_) => serde_json::Value::Null,
    }
}

#[test]
fn a_strict_codex_sandbox_denies_the_kits_extra_hosts_not_in_its_allow_list() {
    let env = Env::new();
    env.write_profile(
        "default",
        &strict(&PROFILE.replace("[\"crates.io\"]", "[\"crates.io\", \"github.com\"]")),
    );
    let denied: Vec<&str> = CODEX_EXTRA[1..].to_vec();
    let backend = sbx_046().with_network_rules(codex_rules(&[], &denied));
    build_codex_template(&env, &backend);

    new_codex(&env, &backend).unwrap();

    let create = backend.creates().pop().unwrap();
    assert!(create.template.is_some());
    assert_eq!(create.deny_network, denied);
    let log = backend.log();
    assert_eq!(
        log[log.len() - 2..],
        ["create sbxm-demo-codex", "network_rules sbxm-demo-codex"]
    );
    assert!(codex_state(&env)["template"].is_string());
}

#[test]
fn a_strict_codex_sandbox_whose_kit_allows_an_unknown_host_is_removed_and_refused() {
    let env = Env::new();
    env.write_profile("default", &strict(PROFILE));
    let backend =
        sbx_046().with_network_rules(codex_rules(&["telemetry.example.com:443"], &CODEX_EXTRA));
    build_codex_template(&env, &backend);

    let err = new_codex(&env, &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains(
            "the network rule 'kit:sbxm-demo-codex' of sbxm-demo-codex allows \
             telemetry.example.com, which strict profile 'default' doesn't list in [template].allow and sbxm doesn't deny; sbxm removed the \
             sandbox. Add the host to [template].allow if codex may reach it, or set strict = false"
        ),
        "{message}"
    );
    let log = backend.log();
    assert_eq!(
        log[log.len() - 3..],
        [
            "create sbxm-demo-codex",
            "network_rules sbxm-demo-codex",
            "rm sbxm-demo-codex"
        ]
    );
    assert_eq!(codex_state(&env), serde_json::Value::Null);
}

#[test]
fn a_host_the_kit_allows_but_a_rule_denies_or_the_allow_list_names_passes() {
    let env = Env::new();
    env.write_profile(
        "default",
        &strict(&PROFILE.replace(
            "[\"crates.io\"]",
            "[\"crates.io\", \"telemetry.example.com\"]",
        )),
    );
    let backend = sbx_046().with_network_rules(codex_rules(
        &["telemetry.example.com:443", "crates.io:443"],
        &CODEX_EXTRA,
    ));
    build_codex_template(&env, &backend);

    new_codex(&env, &backend).unwrap();
}

/// Review PR 269 M-1: `*.x` in `[template].allow` covers every host under `x`, not `x` itself.
const WILDCARD_ALLOW: &str = "[\"crates.io\", \"*.githubusercontent.com\", \"*.github.com\"]";

#[test]
fn a_wildcard_in_the_allow_list_keeps_its_hosts_out_of_the_deny_rules() {
    let env = Env::new();
    env.write_profile(
        "default",
        &strict(&PROFILE.replace("[\"crates.io\"]", WILDCARD_ALLOW)),
    );
    let denied = [
        "github.com",
        "registry.npmjs.org",
        "download.docker.com",
        "archive.ubuntu.com",
        "security.ubuntu.com",
        "ports.ubuntu.com",
    ];
    let backend = sbx_046().with_network_rules(codex_rules(&[], &denied));
    build_codex_template(&env, &backend);

    new_codex(&env, &backend).unwrap();

    assert_eq!(backend.creates().pop().unwrap().deny_network, denied);
}

#[test]
fn a_wildcard_in_the_allow_list_does_not_pass_an_unrelated_kit_host() {
    let env = Env::new();
    env.write_profile(
        "default",
        &strict(&PROFILE.replace("[\"crates.io\"]", WILDCARD_ALLOW)),
    );
    let denied = ["github.com", "registry.npmjs.org", "download.docker.com"];
    let backend = sbx_046().with_network_rules(codex_rules(
        &["archive.ubuntu.com", "telemetry.example.com"],
        &denied,
    ));
    build_codex_template(&env, &backend);

    let message = format!("{:#}", new_codex(&env, &backend).unwrap_err());

    assert!(message.contains("allows archive.ubuntu.com"), "{message}");
    assert_eq!(backend.log().last().unwrap(), "rm sbxm-demo-codex");
}

/// Review PR 269 M-2: a strict `codex` template built for `default`, and a fresh backend that
/// knows its image, for a create whose rule check or removal fails.
fn strict_codex_template(env: &Env) -> FakeBackend {
    use sbxm::backend::SandboxBackend;
    env.write_profile("default", &strict(PROFILE));
    let builder = sbx_046();
    build_codex_template(env, &builder);
    sbx_046().with_templates(builder.template_ls().unwrap())
}

fn assert_removal_failed(message: &str) {
    assert!(
        message.contains("removing it failed; remove it with `sbx rm sbxm-demo-codex`"),
        "{message}"
    );
    assert!(!message.contains("sbxm removed the sandbox"), "{message}");
}

#[test]
fn unreadable_network_rules_remove_the_strict_codex_sandbox() {
    let env = Env::new();
    let backend = strict_codex_template(&env).failing_network_rules();

    let message = format!("{:#}", new_codex(&env, &backend).unwrap_err());

    assert!(message.contains("fake network_rules failure"), "{message}");
    assert!(message.contains("sbxm removed the sandbox"), "{message}");
    let log = backend.log();
    assert_eq!(
        log[log.len() - 2..],
        ["network_rules sbxm-demo-codex", "rm sbxm-demo-codex"]
    );
    assert_eq!(codex_state(&env), serde_json::Value::Null);
}

#[test]
fn a_failed_removal_after_unreadable_rules_keeps_the_cause_and_names_sbx_rm() {
    let env = Env::new();
    let backend = strict_codex_template(&env)
        .failing_network_rules()
        .and_failing_remove();

    let message = format!("{:#}", new_codex(&env, &backend).unwrap_err());

    assert!(message.contains("fake network_rules failure"), "{message}");
    assert_removal_failed(&message);
}

#[test]
fn a_failed_removal_after_a_missing_kit_rule_keeps_the_reason_and_names_sbx_rm() {
    let env = Env::new();
    let backend = strict_codex_template(&env)
        .with_network_rules(Vec::new())
        .and_failing_remove();

    let message = format!("{:#}", new_codex(&env, &backend).unwrap_err());

    assert!(
        message.contains("sbx reported no network rule 'kit:sbxm-demo-codex'"),
        "{message}"
    );
    assert_removal_failed(&message);
}

#[test]
fn a_failed_removal_after_an_unexpected_host_keeps_the_reason_and_names_sbx_rm() {
    let env = Env::new();
    let backend = strict_codex_template(&env)
        .with_network_rules(codex_rules(&["telemetry.example.com"], &CODEX_EXTRA))
        .and_failing_remove();

    let message = format!("{:#}", new_codex(&env, &backend).unwrap_err());

    assert!(
        message.contains("allows telemetry.example.com"),
        "{message}"
    );
    assert_removal_failed(&message);
}

#[test]
fn only_a_strict_codex_sandbox_gets_deny_rules_and_the_check() {
    for (harness, profile) in [
        (Harness::Codex, PROFILE.to_owned()),
        (Harness::Claude, strict(PROFILE)),
    ] {
        let env = Env::new();
        env.write_profile("default", &profile);
        let backend = sbx_046();
        template_build::run(
            &env.config_dir(),
            &template_build::Options {
                profile: "default".into(),
                harness,
                ..Default::default()
            },
            &backend,
            &sbxm::task::record::SystemProbe,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        new::run(
            &env.config_dir(),
            "demo",
            &new::Options {
                harness,
                ..Default::default()
            },
            &backend,
            &mut Vec::new(),
        )
        .unwrap();

        let create = backend.creates().pop().unwrap();
        assert!(create.template.is_some(), "{harness:?}");
        assert!(create.deny_network.is_empty(), "{harness:?}");
        assert!(
            !backend.log().iter().any(|l| l.starts_with("network_rules")),
            "{harness:?}"
        );
    }
}

// Slice 6 review M3S6-2: the check runs even when `[template].allow` names every extra host.
#[test]
fn a_strict_codex_sandbox_allowing_every_extra_host_is_still_checked() {
    let env = Env::new();
    let all = CODEX_EXTRA
        .iter()
        .map(|h| format!("\"{h}\""))
        .collect::<Vec<_>>()
        .join(", ");
    env.write_profile(
        "default",
        &strict(&PROFILE.replace("[\"crates.io\"]", &format!("[\"crates.io\", {all}]"))),
    );
    let backend = sbx_046().with_network_rules(codex_rules(&["telemetry.example.com:443"], &[]));
    build_codex_template(&env, &backend);

    let err = new_codex(&env, &backend).unwrap_err();

    assert!(
        format!("{err:#}").contains("allows telemetry.example.com"),
        "{err:#}"
    );
    assert!(backend.creates().pop().unwrap().deny_network.is_empty());
    assert_eq!(codex_state(&env), serde_json::Value::Null);
}

// Slice 6 review M3S6-1, decision 231: Pi's kit allows registry.npmjs.org, which strict mode can't
// close yet.
#[test]
fn a_strict_profile_is_refused_with_pi_before_any_write() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let backend = sbx_046();
    template_build::run(
        &env.config_dir(),
        &template_build::Options {
            profile: "default".into(),
            harness: Harness::Pi,
            ..Default::default()
        },
        &backend,
        &sbxm::task::record::SystemProbe,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap();
    env.write_profile("default", &strict(PROFILE));
    let creates = backend.creates().len();

    let err = new::run(
        &env.config_dir(),
        "demo",
        &new::Options {
            harness: Harness::Pi,
            ..Default::default()
        },
        &backend,
        &mut Vec::new(),
    )
    .unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains(
            "strict profile 'default' can't be used with the pi harness yet: pi's own kit allows \
             registry.npmjs.org, which strict mode can't close (decision 231); set strict = false \
             in the profile, or use --harness claude or codex"
        ),
        "{message}"
    );
    assert_eq!(backend.creates().len(), creates);
    assert!(!env.base_dir().join("demo").exists());
    assert!(!env.base_dir().join(".sbxm").join("demo").exists());
}

// Decision 232: the check fails closed and covers every allow rule scoped to the sandbox.

/// A strict codex profile with its template built, checked against `rules`.
fn strict_codex_with(rules: Vec<NetworkRule>) -> (Env, FakeBackend, anyhow::Result<()>) {
    let env = Env::new();
    env.write_profile("default", &strict(PROFILE));
    let backend = sbx_046().with_network_rules(rules);
    build_codex_template(&env, &backend);
    let result = new_codex(&env, &backend);
    (env, backend, result)
}

fn assert_removed(env: &Env, backend: &FakeBackend) {
    assert_eq!(backend.log().last().unwrap(), "rm sbxm-demo-codex");
    assert_eq!(codex_state(env), serde_json::Value::Null);
}

#[test]
fn a_strict_codex_sandbox_without_a_kit_rule_is_removed_and_refused() {
    let rules = codex_rules(&[], &CODEX_EXTRA)
        .into_iter()
        .filter(|rule| !rule.allow)
        .collect();

    let (env, backend, result) = strict_codex_with(rules);

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains(
            "sbx reported no network rule 'kit:sbxm-demo-codex' for sbxm-demo-codex, so sbxm can't \
             see what the codex kit allows; sbxm removed the sandbox. Set strict = false, or update \
             sbxm if a newer sbx renamed the rule"
        ),
        "{message}"
    );
    assert_removed(&env, &backend);
}

#[test]
fn another_allow_rule_scoped_to_the_sandbox_is_checked_too() {
    let mut rules = codex_rules(&[], &CODEX_EXTRA);
    rules.push(NetworkRule {
        name: "added-by-hand".into(),
        scope: SCOPE.into(),
        allow: true,
        resources: vec!["example.org".into()],
    });

    let (env, backend, result) = strict_codex_with(rules);

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("the network rule 'added-by-hand' of sbxm-demo-codex allows example.org,"),
        "{message}"
    );
    assert_removed(&env, &backend);
}

#[test]
fn a_global_allow_rule_is_left_to_the_user() {
    let mut rules = codex_rules(&[], &CODEX_EXTRA);
    rules.push(NetworkRule {
        name: "users-own".into(),
        scope: "global".into(),
        allow: true,
        resources: vec!["example.org".into()],
    });

    let (_env, _backend, result) = strict_codex_with(rules);

    result.unwrap();
}
