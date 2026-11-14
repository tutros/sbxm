//! Issue #266, decision 323: a strict `pi` sandbox from a template denies the Pi kit's extra hosts
//! and is checked after its creation, as decision 221 does for `codex`.

mod common;

use common::Env;
use sbxm::backend::{FakeBackend, NetworkRule};
use sbxm::commands::{new, template_build};
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

/// `PROFILE`, strict, with `[template].allow` set to exactly `hosts`.
fn strict_allowing(hosts: &[&str]) -> String {
    let list: Vec<String> = hosts.iter().map(|h| format!("\"{h}\"")).collect();
    strict(&PROFILE.replace("[\"crates.io\"]", &format!("[{}]", list.join(", "))))
}

/// The network rules of `sbx policy ls --json` for a strict `pi` sandbox (`sbx` 0.46.0, captured
/// 2026-11-13 for issue #266), renamed to `sbxm-demo-pi`.
fn fixture_rules() -> Vec<NetworkRule> {
    let text = include_str!("../src/backend/fixtures/sbx-policy-ls-pi.json")
        .replace("sbxm-spike266b-pi", "sbxm-demo-pi");
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    json["rules"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|rule| rule["resource_type"] == "network")
        .map(|rule| NetworkRule {
            name: rule["name"].as_str().unwrap().into(),
            scope: rule["scope"].as_str().unwrap().into(),
            allow: rule["decision"] == "allow",
            resources: rule["resources"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r.as_str().unwrap().into())
                .collect(),
        })
        .collect()
}

/// The fixture's rules without the deny rules for `hosts`, as `sbx` reports them when sbxm didn't
/// deny those hosts.
fn rules_without_denies(hosts: &[&str]) -> Vec<NetworkRule> {
    fixture_rules()
        .into_iter()
        .filter(|rule| rule.allow || !hosts.contains(&rule.resources[0].as_str()))
        .collect()
}

fn backend(rules: Vec<NetworkRule>) -> FakeBackend {
    FakeBackend::default()
        .with_version("0.46.0")
        .with_network_rules(rules)
}

/// Builds `default`'s template for pi from `build_profile`, then writes `profile` for `new`.
fn build_pi_template(env: &Env, backend: &FakeBackend, build_profile: &str, profile: &str) {
    env.write_profile("default", build_profile);
    template_build::run(
        &env.config_dir(),
        &template_build::Options {
            profile: "default".into(),
            harness: Harness::Pi,
            ..Default::default()
        },
        backend,
        &sbxm::task::record::SystemProbe,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap();
    env.write_profile("default", profile);
}

fn new_pi(env: &Env, backend: &FakeBackend) -> anyhow::Result<()> {
    new::run(
        &env.config_dir(),
        "demo",
        &new::Options {
            harness: Harness::Pi,
            ..Default::default()
        },
        backend,
        &mut Vec::new(),
    )
}

fn pi_state(env: &Env) -> serde_json::Value {
    let path = env.base_dir().join(".sbxm").join("demo").join("state.json");
    match std::fs::read_to_string(path) {
        Ok(text) => {
            serde_json::from_str::<serde_json::Value>(&text).unwrap()["sandboxes"]["pi"].clone()
        }
        Err(_) => serde_json::Value::Null,
    }
}

/// A strict `pi` profile allowing `allowed`, with its template built; `new` is run against
/// `rules` and the hosts it denied are returned with its result.
fn strict_pi(allowed: &[&str], rules: Vec<NetworkRule>) -> (Env, FakeBackend, anyhow::Result<()>) {
    let env = Env::new();
    let backend = backend(rules);
    let profile = strict_allowing(allowed);
    build_pi_template(&env, &backend, &profile, &profile);
    let result = new_pi(&env, &backend);
    (env, backend, result)
}

fn denied(backend: &FakeBackend) -> Vec<String> {
    backend.creates().pop().unwrap().deny_network
}

#[test]
fn a_strict_pi_sandbox_with_an_empty_allow_list_denies_both_extra_hosts() {
    let (env, backend, result) = strict_pi(&[], fixture_rules());

    let profile = config::Profile::load(&env.profiles_dir(), "default").unwrap();
    assert!(profile.template.unwrap().allow.is_empty());
    result.unwrap();
    assert_eq!(
        denied(&backend),
        ["registry.npmjs.org", "platform.claude.com:443"]
    );
    assert!(backend.creates().pop().unwrap().template.is_some());
    assert!(
        backend
            .log()
            .contains(&"network_rules sbxm-demo-pi".to_owned())
    );
    assert!(pi_state(&env)["template"].is_string());
}

#[test]
fn a_strict_pi_sandbox_allowing_registry_npmjs_org_denies_only_platform_claude_com() {
    let (_env, backend, result) = strict_pi(
        &["registry.npmjs.org"],
        rules_without_denies(&["registry.npmjs.org"]),
    );

    result.unwrap();
    assert_eq!(denied(&backend), ["platform.claude.com:443"]);
}

#[test]
fn a_strict_pi_sandbox_allowing_both_extra_hosts_denies_none_and_is_still_checked() {
    let (_env, backend, result) = strict_pi(
        &["registry.npmjs.org", "platform.claude.com"],
        rules_without_denies(&["registry.npmjs.org", "platform.claude.com:443"]),
    );

    result.unwrap();
    assert!(denied(&backend).is_empty());
    assert!(
        backend
            .log()
            .contains(&"network_rules sbxm-demo-pi".to_owned())
    );
}

// The kit writes `platform.claude.com:443`; `[template].allow` names hosts, and the rules check
// compares the kit's host without its port, so an allow entry without the port keeps the host.
#[test]
fn allowing_platform_claude_com_without_the_port_keeps_it() {
    let (_env, backend, result) = strict_pi(
        &["platform.claude.com"],
        rules_without_denies(&["platform.claude.com:443"]),
    );

    result.unwrap();
    assert_eq!(denied(&backend), ["registry.npmjs.org"]);
}

// An allow entry with the port matches no host (the existing `allows` rule compares bare hosts),
// so the host is still denied and the check passes.
#[test]
fn allowing_platform_claude_com_with_the_port_still_denies_it() {
    let (_env, backend, result) = strict_pi(&["platform.claude.com:443"], fixture_rules());

    result.unwrap();
    assert_eq!(
        denied(&backend),
        ["registry.npmjs.org", "platform.claude.com:443"]
    );
}

#[test]
fn a_strict_pi_sandbox_whose_kit_allows_an_unknown_host_is_removed_and_refused() {
    let mut rules = fixture_rules();
    rules
        .iter_mut()
        .find(|rule| rule.name == "kit:sbxm-demo-pi")
        .unwrap()
        .resources
        .push("telemetry.example.com:443".into());

    let (env, backend, result) = strict_pi(&[], rules);

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains(
            "the network rule 'kit:sbxm-demo-pi' of sbxm-demo-pi allows telemetry.example.com, \
             which strict profile 'default' doesn't list in [template].allow and sbxm doesn't \
             deny; sbxm removed the sandbox. Add the host to [template].allow if pi may reach it, \
             or set strict = false"
        ),
        "{message}"
    );
    assert_eq!(backend.log().last().unwrap(), "rm sbxm-demo-pi");
    assert_eq!(pi_state(&env), serde_json::Value::Null);
}

#[test]
fn a_strict_pi_sandbox_without_a_kit_rule_names_the_pi_kit() {
    let rules = fixture_rules().into_iter().filter(|r| !r.allow).collect();

    let (_env, backend, result) = strict_pi(&[], rules);

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("so sbxm can't see what the pi kit allows; sbxm removed the sandbox."),
        "{message}"
    );
    assert_eq!(backend.log().last().unwrap(), "rm sbxm-demo-pi");
}

#[test]
fn a_non_strict_pi_sandbox_and_one_without_a_template_get_no_denies_and_no_check() {
    let without_template = PROFILE.split("[template]").next().unwrap().to_owned();
    for (build_profile, profile) in [
        (PROFILE.to_owned(), PROFILE.to_owned()),
        (PROFILE.to_owned(), without_template),
    ] {
        let env = Env::new();
        let backend = backend(fixture_rules());
        build_pi_template(&env, &backend, &build_profile, &profile);

        new_pi(&env, &backend).unwrap();

        assert!(denied(&backend).is_empty(), "{profile}");
        assert!(
            !backend.log().iter().any(|l| l.starts_with("network_rules")),
            "{profile}"
        );
    }
}
