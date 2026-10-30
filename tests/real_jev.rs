//! Issue 285 (decision 241) against the real `sbx`, both `#[ignore]`d:
//!
//! - `SBXM_REAL_BASE_DIR=<existing dir, not on C:> cargo test --test real_jev scoped -- --ignored`
//!   creates a custom secret scoped to one sandbox, from a helper printing a **fake** token, and
//!   proves on a header-echo host (`httpbin.org`) that the proxy swaps the placeholder in a header
//!   and nowhere else; then removes the sandbox and the secret. Needs `sbx` 0.46.0 or newer.
//! - Also set `SBXM_TEST_JEV_TOKEN_COMMAND=<absolute path to a helper printing the real token>` to
//!   run `live_jev_call`: one real Jev call against a two-criterion rubric.
//!
//! Every name carries `sbxm-it-<pid>`; cleanup runs in `Drop`, even when an assertion fails.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sbxm::backend::{
    CreateSpec, CustomSecretSpec, ExecSpec, SandboxBackend, SbxBackend, SecretSource, SkillsStore,
    Stdin,
};
use sbxm::config::{Profile, Resources};
use sbxm::eval::jev;
use sbxm::headless::{HeadlessResult, RunStatus, Usage};
use sbxm::kit;
use sbxm::run::config::RunConfig;
use sbxm::run::diff::Diff;
use sbxm::run::id::{self, RunRoots};
use sbxm::run::orchestrate::PairOutcome;

/// Removes the sandbox, its scoped secret and the folders, even if the test fails.
struct Cleanup {
    sandbox: String,
    placeholder: String,
    dirs: Vec<PathBuf>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        // Usually already gone (the test removes both), so hide sbx's "not found".
        for args in [
            vec!["rm", "-f", &self.sandbox],
            vec![
                "secret",
                "rm",
                "--placeholder",
                &self.placeholder,
                "--sandbox",
                &self.sandbox,
                "-f",
            ],
        ] {
            let _ = Command::new("sbx")
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        for dir in &self.dirs {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn real_base_dir() -> PathBuf {
    let dir = std::env::var_os("SBXM_REAL_BASE_DIR")
        .expect("set SBXM_REAL_BASE_DIR to an existing dir, not on C: (decision 56)");
    PathBuf::from(dir)
}

/// A helper printing `token`, outside any workspace and the temp dir, as `sbx` requires.
fn write_helper(dir: &Path, token: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    if cfg!(windows) {
        let path = dir.join("token.cmd");
        std::fs::write(&path, format!("@echo {token}\r\n")).unwrap();
        path
    } else {
        let path = dir.join("token.sh");
        std::fs::write(&path, format!("#!/bin/sh\necho {token}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        path
    }
}

fn sh(backend: &SbxBackend, sandbox: &str, command: &str) -> String {
    let output = backend
        .exec(
            sandbox,
            &ExecSpec {
                workdir: None,
                argv: ["sh", "-c", command].map(str::to_owned).into(),
                stdin: Stdin::Closed,
            },
        )
        .unwrap();
    println!("{command}\n{}{}", output.stdout, output.stderr);
    output.stdout
}

#[test]
#[ignore = "needs a logged-in sbx 0.46.0+ and SBXM_REAL_BASE_DIR"]
fn scoped_custom_secret_swaps_the_placeholder_in_the_header_only() {
    let backend = SbxBackend::LOCAL;
    let base = real_base_dir();
    let name = format!("sbxm-it-{}-jev", std::process::id());
    let sandbox = format!("sbxm-{name}");
    let placeholder = format!("{name}-placeholder");
    let token = format!("SBXM-IT-FAKE-TOKEN-{}", std::process::id());
    let root = base.join(&name);
    let _cleanup = Cleanup {
        sandbox: sandbox.clone(),
        placeholder: placeholder.clone(),
        dirs: vec![root.clone()],
    };
    let helper = write_helper(&root.join("helper"), &token);
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let mut profile = Profile::default();
    profile.network.allow = vec!["httpbin.org".into()];
    let kit_dir = root.join("kit").join("common");
    kit::write(&kit_dir, &kit::common("jev-it", &profile, "jev-it")).unwrap();

    backend
        .set_custom_secret(&CustomSecretSpec {
            sandbox: sandbox.clone(),
            hosts: vec!["httpbin.org".into()],
            env: jev::ENV.into(),
            placeholder: placeholder.clone(),
            source: SecretSource::Command(helper),
        })
        .unwrap();
    let listed = backend.custom_secrets().unwrap();
    assert!(
        listed
            .iter()
            .any(|s| s.placeholder == placeholder && !s.is_global()),
        "{listed:?}"
    );
    backend
        .create(&CreateSpec {
            name: sandbox.clone(),
            agent: jev::AGENT.into(),
            workspace: Some(workspace.clone()),
            cpus: 2,
            memory: "2g".into(),
            skills: SkillsStore::Off,
            kits: vec![kit_dir],
            template: None,
            deny_network: vec![],
            ttl: None,
        })
        .unwrap();

    // The sandbox holds the placeholder only.
    assert_eq!(
        sh(&backend, &sandbox, "printenv JEV_API_KEY").trim(),
        placeholder
    );
    // Swapped in a header to the named host.
    let header = sh(
        &backend,
        &sandbox,
        "curl -sS -H \"Authorization: Bearer $JEV_API_KEY\" https://httpbin.org/headers",
    );
    assert!(header.contains(&format!("Bearer {token}")), "{header}");
    // Not in a body or a query string.
    let body = sh(
        &backend,
        &sandbox,
        "curl -sS -X POST -d \"k=$JEV_API_KEY\" \"https://httpbin.org/anything?q=$JEV_API_KEY\"",
    );
    assert!(!body.contains(&token), "{body}");
    assert!(body.contains(&placeholder), "{body}");
    // Nowhere in the sandbox's files or environment.
    let found = sh(
        &backend,
        &sandbox,
        &format!(
            "env | grep -c {token}; grep -rl {token} / --exclude-dir=proc --exclude-dir=sys \
             --exclude-dir=dev 2>/dev/null | head -1"
        ),
    );
    assert_eq!(found.trim(), "0", "{found}");

    backend.remove(&sandbox).unwrap();
    backend
        .remove_custom_secret(&placeholder, &sandbox)
        .unwrap();
    let left = backend.custom_secrets().unwrap();
    assert!(
        !left.iter().any(|s| s.placeholder == placeholder),
        "{left:?}"
    );
    // A second removal answers "no custom secret found", which is not an error.
    backend
        .remove_custom_secret(&placeholder, &sandbox)
        .unwrap();
}

#[test]
#[ignore = "needs a logged-in sbx 0.46.0+, SBXM_REAL_BASE_DIR and SBXM_TEST_JEV_TOKEN_COMMAND"]
fn live_jev_call() {
    let Some(token_command) = std::env::var_os("SBXM_TEST_JEV_TOKEN_COMMAND") else {
        eprintln!("SBXM_TEST_JEV_TOKEN_COMMAND is not set; skipping the live Jev call");
        return;
    };
    let backend = SbxBackend::LOCAL;
    let base = real_base_dir();
    let roots: RunRoots = id::reserve(&base).unwrap();
    let (sandbox, placeholder) = jev::names(&roots.id, 0);
    let _cleanup = Cleanup {
        sandbox,
        placeholder,
        dirs: vec![roots.meta.clone(), roots.workspaces.clone()],
    };
    let config_path = roots.meta.join("run-config.toml");
    let command = toml::Value::String(PathBuf::from(token_command).display().to_string());
    std::fs::write(
        &config_path,
        format!(
            "[task]\nprompt = \"Write a Rust function that adds two numbers.\"\n\n\
             [[contestants]]\nharness = \"claude\"\nmodel = \"a\"\n\n\
             [[contestants]]\nharness = \"claude\"\nmodel = \"b\"\n\n\
             [[eval.rubric]]\nid = \"correct\"\nkind = \"pass_fail\"\nweight = 1.0\nnotes = \"the function returns a + b\"\n\n\
             [[eval.rubric]]\nid = \"quality\"\nkind = \"scale\"\nlevels = [\"poor\", \"fair\", \"good\"]\nweight = 0.5\n\n\
             [eval.jev]\ntoken_command = {command}\n"
        ),
    )
    .unwrap();
    let run_config = RunConfig::load(&config_path).unwrap();
    jev::preflight(run_config.eval.jev.as_ref().unwrap(), &backend).unwrap();
    let kit = jev::build_kit(
        &roots.meta.join("kits"),
        &Resources {
            cpus: 2,
            memory: "2g".into(),
        },
        &backend,
    )
    .unwrap();
    let pair = PairOutcome {
        contestant: 0,
        repeat: 0,
        profile: "default".into(),
        sandbox: "none".into(),
        workspace: roots.workspaces.join("0").join("0"),
        result: Ok(HeadlessResult {
            status: RunStatus::Completed,
            answer: "I added `fn add(a: i32, b: i32) -> i32 { a + b }`.".into(),
            transcript: String::new(),
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: None,
            },
        }),
        diff: Some(Ok(Diff {
            patch: "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -0,0 +1 @@\n+pub fn add(a: i32, b: i32) -> i32 { a + b }\n".into(),
            skipped: vec![],
        })),
        checks: vec![],
        remove_error: None,
        save_error: None,
    };
    let run = jev::evaluate_repeat(
        &backend,
        &run_config,
        &kit,
        &roots,
        0,
        &[&pair],
        &std::thread::sleep,
    )
    .unwrap();
    println!("{run:#?}");
    assert_eq!(run.remove_error, None);
    assert_eq!(run.secret_remove_error, None);
    let scores = run.results[0].1.as_ref().unwrap();
    assert!(scores.answered.model.starts_with("jev-"), "{scores:?}");
    assert!(scores.answered.unscored.is_empty(), "{scores:?}");
    assert_eq!(scores.chunks, 1);
}
