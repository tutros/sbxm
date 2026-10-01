//! Tests against the real `sbx`. Run with:
//! `SBXM_REAL_BASE_DIR=<existing dir, not on C: (decision 56)> cargo test --test real_sbx -- --ignored`
//! Needs a logged-in `sbx` with the `anthropic` secret stored.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use sbxm::backend::{SandboxBackend, SbxBackend};
use sbxm::commands::{list, new, rm, stop};
use sbxm::confirm::Terminal;
use tempfile::TempDir;

/// Removes the sandbox, workspace and metadata even if the test fails.
struct Cleanup {
    sandbox: String,
    dirs: Vec<PathBuf>,
    /// Removed only if empty.
    shared_dirs: Vec<PathBuf>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        // Usually already gone (the test removes it), so hide sbx's "not found".
        let _ = Command::new("sbx")
            .args(["rm", "-f", &self.sandbox])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        for dir in &self.dirs {
            let _ = std::fs::remove_dir_all(dir);
        }
        for dir in &self.shared_dirs {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

/// `printenv <name>` inside the sandbox, trimmed.
fn printenv(sandbox: &str, name: &str) -> String {
    let output = Command::new("sbx")
        .args(["exec", sandbox, "printenv", name])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// The HTTP status a request from inside the sandbox gets. The egress proxy
/// answers blocked hosts with 403, reported by curl as the CONNECT status.
fn http_status(sandbox: &str, url: &str) -> String {
    let output = Command::new("sbx")
        .args(["exec", sandbox, "curl", "-s", "-o", "/dev/null", "-w"])
        .arg("%{http_code} %{http_connect}")
        .arg(url)
        .output()
        .unwrap();
    let codes = String::from_utf8_lossy(&output.stdout).into_owned();
    println!("{url}: {codes}");
    let mut parts = codes.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some("000"), Some(connect)) => connect.to_owned(),
        (Some(code), _) => code.to_owned(),
        _ => codes,
    }
}

fn real_base_dir() -> PathBuf {
    let dir = std::env::var_os("SBXM_REAL_BASE_DIR")
        .expect("set SBXM_REAL_BASE_DIR to an existing dir, not on C: (decision 56)");
    PathBuf::from(dir)
}

#[test]
#[ignore = "needs a logged-in sbx and SBXM_REAL_BASE_DIR"]
fn lifecycle_against_real_sbx() {
    let base_dir = real_base_dir();
    let project = format!("sbxm-it-{}", std::process::id());
    let sandbox = format!("sbxm-{project}-claude");
    let _cleanup = Cleanup {
        sandbox: sandbox.clone(),
        dirs: vec![
            base_dir.join(&project),
            base_dir.join(".sbxm").join(&project),
        ],
        shared_dirs: vec![base_dir.join(".sbxm")],
    };
    let config_dir = TempDir::new().unwrap();
    let base = toml::Value::String(base_dir.to_str().unwrap().to_owned());
    std::fs::write(
        config_dir.path().join("config.toml"),
        format!("base_dir = {base}\n\n[resources]\ncpus = 2\nmemory = \"2g\"\n"),
    )
    .unwrap();
    // The default profile, at the default `<config_dir>/profiles` location.
    let profile_dir = config_dir.path().join("profiles").join("default");
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join("profile.toml"),
        "description = \"real sbx test\"\n\n[network]\nallow = [\"example.org\"]\n\n[env]\nREAL_TEST_GREETING = \"hello from the profile\"\n\n[secrets]\nservices = [\"anthropic\"]\n\n[instructions]\nmandatory = \"mandatory.md\"\n\n[[setup.install]]\ncommand = \"echo ran-as-$(id -un) > /tmp/sbxm-install\"\nuser = \"agent\"\n\n[harness.claude]\nhome_files = \"home\"\nmanaged_settings = \"managed.json\"\n",
    )
    .unwrap();
    std::fs::write(
        profile_dir.join("mandatory.md"),
        "REAL TEST CANARY: the word is QUINCE-5\n",
    )
    .unwrap();
    let agents_dir = profile_dir.join("home").join(".claude").join("agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(agents_dir.join("real-test.md"), "REAL TEST HOME FILE\n").unwrap();
    std::fs::write(
        profile_dir.join("managed.json"),
        r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "echo $HOME > /tmp/sbxm-hook"}]}]}}"#,
    )
    .unwrap();

    new::run(
        config_dir.path(),
        &project,
        &new::Options::default(),
        &SbxBackend,
        &mut std::io::stderr(),
    )
    .unwrap();

    // Issue #2: a second `new` is refused by sbxm, before `sbx create` runs.
    let again = new::run(
        config_dir.path(),
        &project,
        &new::Options::default(),
        &SbxBackend,
        &mut std::io::stderr(),
    )
    .unwrap_err();
    assert_eq!(
        format!("{again:#}"),
        format!(
            "sandbox {sandbox} already exists; open it with `sbxm open {project} --harness claude`"
        )
    );

    let ls = Command::new("sbx").args(["ls", "--json"]).output().unwrap();
    let ls = String::from_utf8(ls.stdout).unwrap();
    assert!(
        ls.contains(&format!("\"{sandbox}\"")),
        "sbx ls --json: {ls}"
    );

    // Slice 10b: the profile's allow list reaches the sandbox; other hosts stay blocked.
    assert_eq!(http_status(&sandbox, "https://example.org"), "200");
    assert_eq!(http_status(&sandbox, "https://example.com"), "403");

    // Slice 14: the mandatory instructions are Claude's user-level CLAUDE.md.
    let claude_md = Command::new("sbx")
        .args(["exec", &sandbox, "cat", "/home/agent/.claude/CLAUDE.md"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&claude_md.stdout).trim(),
        "REAL TEST CANARY: the word is QUINCE-5"
    );

    // Slice 15a: the profile's install step ran, as the user it names.
    let install = Command::new("sbx")
        .args(["exec", &sandbox, "cat", "/tmp/sbxm-install"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&install.stdout).trim(),
        "ran-as-agent"
    );

    // Slice 15b: the profile's home files are in the agent's home.
    let home_file = Command::new("sbx")
        .args([
            "exec",
            &sandbox,
            "cat",
            "/home/agent/.claude/agents/real-test.md",
        ])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&home_file.stdout).trim(),
        "REAL TEST HOME FILE"
    );

    // Slice 15c: managed settings are root-owned, and the escaped `$` decodes.
    let managed = "/etc/claude-code/managed-settings.json";
    let owner = Command::new("sbx")
        .args(["exec", &sandbox, "stat", "-c", "%U:%a", managed])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&owner.stdout).trim(), "root:644");
    let hook = Command::new("sbx")
        .args([
            "exec",
            &sandbox,
            "jq",
            "-r",
            ".hooks.SessionStart[0].hooks[0].command",
            managed,
        ])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&hook.stdout).trim(),
        "echo $HOME > /tmp/sbxm-hook"
    );

    // Slice 10c: the profile's env is set inside the sandbox.
    assert_eq!(
        printenv(&sandbox, "REAL_TEST_GREETING"),
        "hello from the profile"
    );

    // Slice 11: the sandbox records the profile and config hash from state.
    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(base_dir.join(".sbxm").join(&project).join("state.json")).unwrap(),
    )
    .unwrap();
    let hash = state["sandboxes"]["claude"]["config_hash"]
        .as_str()
        .unwrap();
    assert_eq!(printenv(&sandbox, "SBXM_CONFIG_HASH"), hash);
    assert_eq!(printenv(&sandbox, "SBXM_PROFILE"), "default");

    // Slice 11c: a rebuild (what `open --rebuild` does before attaching)
    // recreates the sandbox with the changed config and keeps the workspace.
    let workspace = base_dir.join(&project);
    std::fs::write(workspace.join("keep.txt"), "kept").unwrap();
    std::fs::write(
        profile_dir.join("profile.toml"),
        "[network]\nallow = [\"example.org\"]\n\n[env]\nREAL_TEST_GREETING = \"rebuilt\"\n",
    )
    .unwrap();
    let rebuild = new::Options {
        replace: true,
        ..new::Options::default()
    };
    new::run(
        config_dir.path(),
        &project,
        &rebuild,
        &SbxBackend,
        &mut std::io::stderr(),
    )
    .unwrap();
    assert_eq!(printenv(&sandbox, "REAL_TEST_GREETING"), "rebuilt");
    assert_eq!(
        std::fs::read_to_string(workspace.join("keep.txt")).unwrap(),
        "kept"
    );

    // Slice 5: `list` sees it through the real `sbx ls --json`, joined with state.
    let entries = list::entries(config_dir.path(), &SbxBackend).unwrap();
    let entry = entries
        .iter()
        .find(|e| e.sandbox == sandbox)
        .unwrap_or_else(|| panic!("{sandbox} not in {entries:?}"));
    assert_eq!(entry.project, project);
    assert_eq!(entry.harness, "claude");
    assert_eq!(entry.problem, None);
    // Slice 11d: the rebuilt sandbox matches the current config.
    assert_eq!(entry.config, Some(list::ConfigStatus::Current));
    println!("list status for {sandbox}: {}", entry.status);

    // Slice 6: `stop` stops it, and `list` shows that.
    stop::run(
        config_dir.path(),
        &project,
        sbxm::harness::Harness::Claude,
        &SbxBackend,
    )
    .unwrap();
    let entries = list::entries(config_dir.path(), &SbxBackend).unwrap();
    let entry = entries.iter().find(|e| e.sandbox == sandbox).unwrap();
    assert_eq!(entry.status, "stopped");

    // Slice 7: `rm` removes the sandbox and state but keeps the workspace.
    rm::run(
        config_dir.path(),
        &project,
        &rm::Options::default(),
        &SbxBackend,
        &Terminal,
    )
    .unwrap();
    let entries = list::entries(config_dir.path(), &SbxBackend).unwrap();
    assert!(!entries.iter().any(|e| e.sandbox == sandbox), "{entries:?}");
    assert!(base_dir.join(&project).is_dir());
    assert!(!base_dir.join(".sbxm").join(&project).exists());
}

#[test]
#[ignore = "needs a logged-in sbx and SBXM_REAL_BASE_DIR"]
fn missing_secret_is_refused_before_create() {
    let stored = SbxBackend.secret_services().unwrap();
    // Services `sbx secret set` accepts (v0.43.0); use one that isn't stored.
    let missing = ["xai", "groq", "mistral", "nebius", "openrouter", "devin"]
        .into_iter()
        .find(|s| !stored.iter().any(|t| t == s))
        .expect("every candidate service is stored; add another candidate");
    let base_dir = real_base_dir();
    let project = format!("sbxm-it-secret-{}", std::process::id());
    let _cleanup = Cleanup {
        sandbox: format!("sbxm-{project}-claude"),
        dirs: vec![
            base_dir.join(&project),
            base_dir.join(".sbxm").join(&project),
        ],
        shared_dirs: vec![base_dir.join(".sbxm")],
    };
    let config_dir = TempDir::new().unwrap();
    let base = toml::Value::String(base_dir.to_str().unwrap().to_owned());
    std::fs::write(
        config_dir.path().join("config.toml"),
        format!(
            r#"base_dir = {base}

[resources]
cpus = 2
memory = "2g"
"#
        ),
    )
    .unwrap();
    let profile_dir = config_dir.path().join("profiles").join("default");
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join("profile.toml"),
        format!(
            r#"[secrets]
services = ["{missing}"]
"#
        ),
    )
    .unwrap();

    let err = new::run(
        config_dir.path(),
        &project,
        &new::Options::default(),
        &SbxBackend,
        &mut std::io::stderr(),
    )
    .unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains(&format!(
            "secret '{missing}' (secrets.services) is not stored in sbx"
        )),
        "{message}"
    );
    assert!(!base_dir.join(&project).exists());
}

/// Slice 18a: a Codex sandbox gets the mandatory instructions as
/// `~/.codex/AGENTS.md`, and Codex renders them into its prompt.
/// `codex debug prompt-input` makes no model call, so no secret is needed.
#[test]
#[ignore = "needs a logged-in sbx and SBXM_REAL_BASE_DIR"]
fn codex_mandatory_instructions_against_real_sbx() {
    let base_dir = real_base_dir();
    let project = format!("sbxm-it-{}-codex", std::process::id());
    let sandbox = format!("sbxm-{project}-codex");
    let _cleanup = Cleanup {
        sandbox: sandbox.clone(),
        dirs: vec![
            base_dir.join(&project),
            base_dir.join(".sbxm").join(&project),
        ],
        shared_dirs: vec![base_dir.join(".sbxm")],
    };
    let config_dir = TempDir::new().unwrap();
    let base = toml::Value::String(base_dir.to_str().unwrap().to_owned());
    std::fs::write(
        config_dir.path().join("config.toml"),
        format!("base_dir = {base}\n\n[resources]\ncpus = 2\nmemory = \"2g\"\n"),
    )
    .unwrap();
    let profile_dir = config_dir.path().join("profiles").join("default");
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join("profile.toml"),
        "[instructions]\nmandatory = \"mandatory.md\"\n",
    )
    .unwrap();
    std::fs::write(
        profile_dir.join("mandatory.md"),
        "REAL TEST CANARY: the word is DAMSON-7\n",
    )
    .unwrap();

    let options = new::Options {
        harness: sbxm::harness::Harness::Codex,
        ..Default::default()
    };
    new::run(
        config_dir.path(),
        &project,
        &options,
        &SbxBackend,
        &mut std::io::stderr(),
    )
    .unwrap();

    let agents_md = Command::new("sbx")
        .args(["exec", &sandbox, "cat", "/home/agent/.codex/AGENTS.md"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&agents_md.stdout).trim(),
        "REAL TEST CANARY: the word is DAMSON-7"
    );
    let prompt = Command::new("sbx")
        .args(["exec", &sandbox, "codex", "debug", "prompt-input", "hi"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&prompt.stdout).contains("DAMSON-7"),
        "codex debug prompt-input: {}",
        String::from_utf8_lossy(&prompt.stderr)
    );

    // Slice 18c: stop and rm act on the Codex sandbox.
    let codex = sbxm::harness::Harness::Codex;
    stop::run(config_dir.path(), &project, codex, &SbxBackend).unwrap();
    let rm_codex = rm::Options {
        harness: codex,
        ..Default::default()
    };
    rm::run(
        config_dir.path(),
        &project,
        &rm_codex,
        &SbxBackend,
        &Terminal,
    )
    .unwrap();
    let ls = Command::new("sbx").args(["ls", "--json"]).output().unwrap();
    assert!(!String::from_utf8_lossy(&ls.stdout).contains(&sandbox));
}

/// Slice 19: a Gemini CLI sandbox gets the mandatory instructions as
/// `~/.gemini/GEMINI.md`. Whether Gemini loads it needs Google credentials,
/// so only the file is checked (decision 49).
#[test]
#[ignore = "needs a logged-in sbx and SBXM_REAL_BASE_DIR"]
fn gemini_mandatory_instructions_against_real_sbx() {
    let base_dir = real_base_dir();
    let project = format!("sbxm-it-{}-gemini", std::process::id());
    let sandbox = format!("sbxm-{project}-gemini");
    let _cleanup = Cleanup {
        sandbox: sandbox.clone(),
        dirs: vec![
            base_dir.join(&project),
            base_dir.join(".sbxm").join(&project),
        ],
        shared_dirs: vec![base_dir.join(".sbxm")],
    };
    let config_dir = TempDir::new().unwrap();
    let base = toml::Value::String(base_dir.to_str().unwrap().to_owned());
    std::fs::write(
        config_dir.path().join("config.toml"),
        format!("base_dir = {base}\n\n[resources]\ncpus = 2\nmemory = \"2g\"\n"),
    )
    .unwrap();
    let profile_dir = config_dir.path().join("profiles").join("default");
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join("profile.toml"),
        "[instructions]\nmandatory = \"mandatory.md\"\n",
    )
    .unwrap();
    std::fs::write(
        profile_dir.join("mandatory.md"),
        "REAL TEST CANARY: the word is MEDLAR-3\n",
    )
    .unwrap();
    let gemini = sbxm::harness::Harness::Gemini;
    let options = new::Options {
        harness: gemini,
        ..Default::default()
    };

    new::run(
        config_dir.path(),
        &project,
        &options,
        &SbxBackend,
        &mut std::io::stderr(),
    )
    .unwrap();

    let gemini_md = Command::new("sbx")
        .args(["exec", &sandbox, "cat", "/home/agent/.gemini/GEMINI.md"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&gemini_md.stdout).trim(),
        "REAL TEST CANARY: the word is MEDLAR-3"
    );
    stop::run(config_dir.path(), &project, gemini, &SbxBackend).unwrap();
    let rm_gemini = rm::Options {
        harness: gemini,
        ..Default::default()
    };
    rm::run(
        config_dir.path(),
        &project,
        &rm_gemini,
        &SbxBackend,
        &Terminal,
    )
    .unwrap();
    let ls = Command::new("sbx").args(["ls", "--json"]).output().unwrap();
    assert!(!String::from_utf8_lossy(&ls.stdout).contains(&sandbox));
}

/// Slice 20: a Pi sandbox from the pinned Docker Hub kit gets the mandatory
/// instructions as `~/.pi/agent/AGENTS.md` (S7 showed Pi renders that file into
/// its prompt). Run from a terminal before the user has a credential binding,
/// `sbx create` may ask about it (decision 74).
#[test]
#[ignore = "needs a logged-in sbx and SBXM_REAL_BASE_DIR"]
fn pi_mandatory_instructions_against_real_sbx() {
    let base_dir = real_base_dir();
    let project = format!("sbxm-it-{}-pi", std::process::id());
    let sandbox = format!("sbxm-{project}-pi");
    let _cleanup = Cleanup {
        sandbox: sandbox.clone(),
        dirs: vec![
            base_dir.join(&project),
            base_dir.join(".sbxm").join(&project),
        ],
        shared_dirs: vec![base_dir.join(".sbxm")],
    };
    let config_dir = TempDir::new().unwrap();
    let base = toml::Value::String(base_dir.to_str().unwrap().to_owned());
    std::fs::write(
        config_dir.path().join("config.toml"),
        format!("base_dir = {base}\n\n[resources]\ncpus = 2\nmemory = \"2g\"\n"),
    )
    .unwrap();
    let profile_dir = config_dir.path().join("profiles").join("default");
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join("profile.toml"),
        "[instructions]\nmandatory = \"mandatory.md\"\n",
    )
    .unwrap();
    std::fs::write(
        profile_dir.join("mandatory.md"),
        "REAL TEST CANARY: the word is ROWAN-4\n",
    )
    .unwrap();
    let pi = sbxm::harness::Harness::Pi;
    let options = new::Options {
        harness: pi,
        ..Default::default()
    };

    new::run(
        config_dir.path(),
        &project,
        &options,
        &SbxBackend,
        &mut std::io::stderr(),
    )
    .unwrap();

    let ls = Command::new("sbx").args(["ls", "--json"]).output().unwrap();
    let ls: serde_json::Value = serde_json::from_slice(&ls.stdout).unwrap();
    let entry = ls["sandboxes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == sandbox.as_str())
        .expect("sandbox listed");
    assert_eq!(entry["agent"], "pi");
    let agents_md = Command::new("sbx")
        .args(["exec", &sandbox, "cat", "/home/agent/.pi/agent/AGENTS.md"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&agents_md.stdout).trim(),
        "REAL TEST CANARY: the word is ROWAN-4"
    );
    stop::run(config_dir.path(), &project, pi, &SbxBackend).unwrap();
    let rm_pi = rm::Options {
        harness: pi,
        ..Default::default()
    };
    rm::run(config_dir.path(), &project, &rm_pi, &SbxBackend, &Terminal).unwrap();
    let ls = Command::new("sbx").args(["ls", "--json"]).output().unwrap();
    assert!(!String::from_utf8_lossy(&ls.stdout).contains(&sandbox));
}

#[test]
#[ignore = "needs a logged-in sbx and SBXM_REAL_BASE_DIR"]
fn exec_and_skills_against_real_sbx() {
    use sbxm::backend::{CreateSpec, ExecSpec, SkillsStore, Stdin};

    let base_dir = real_base_dir();
    let name = format!("sbxm-it-{}-exec", std::process::id());
    let workspace = base_dir.join(&name);
    let _cleanup = Cleanup {
        sandbox: name.clone(),
        dirs: vec![workspace.clone()],
        shared_dirs: vec![],
    };
    std::fs::create_dir_all(&workspace).unwrap();
    SbxBackend
        .create(&CreateSpec {
            name: name.clone(),
            agent: "claude".into(),
            workspace,
            cpus: 2,
            memory: "2g".into(),
            skills: SkillsStore::Off,
            kits: vec![],
        })
        .unwrap();

    let run = |workdir: Option<&str>, argv: &[&str], stdin: Stdin| {
        SbxBackend
            .exec(
                &name,
                &ExecSpec {
                    workdir: workdir.map(PathBuf::from),
                    argv: argv.iter().map(|a| a.to_string()).collect(),
                    stdin,
                },
            )
            .unwrap()
    };

    let out = run(None, &["echo", "hi"], Stdin::Closed);
    assert_eq!((out.stdout.trim(), out.exit_code), ("hi", Some(0)));

    // The working directory is passed through.
    let out = run(Some("/tmp"), &["pwd"], Stdin::Closed);
    assert_eq!(out.stdout.trim(), "/tmp");

    // stdout and stderr stay separate, and a non-zero exit is data, not an error.
    let out = run(
        None,
        &["sh", "-c", "echo out; echo err >&2; exit 3"],
        Stdin::Closed,
    );
    assert_eq!(
        (out.stdout.trim(), out.stderr.trim(), out.exit_code),
        ("out", "err", Some(3))
    );

    // Closed stdin and empty piped stdin both give EOF at once; piped text arrives.
    let out = run(None, &["cat"], Stdin::Closed);
    assert_eq!((out.stdout.as_str(), out.exit_code), ("", Some(0)));
    let out = run(None, &["cat"], Stdin::Piped(String::new()));
    assert_eq!((out.stdout.as_str(), out.exit_code), ("", Some(0)));
    let out = run(None, &["cat"], Stdin::Piped("piped input".into()));
    assert_eq!(
        (out.stdout.as_str(), out.exit_code),
        ("piped input", Some(0))
    );

    assert!(SbxBackend.skills().unwrap().is_object());
}

#[test]
#[ignore = "needs a logged-in sbx, the anthropic secret and SBXM_REAL_BASE_DIR"]
fn claude_headless_against_real_sbx() {
    use std::time::Duration;

    use sbxm::backend::{CreateSpec, ExecSpec, SkillsStore, Stdin};
    use sbxm::harness::Harness;
    use sbxm::headless::{self, HeadlessOpts, RunStatus};

    let base_dir = real_base_dir();
    let name = format!("sbxm-it-{}-headless", std::process::id());
    let workspace = base_dir.join(&name);
    let _cleanup = Cleanup {
        sandbox: name.clone(),
        dirs: vec![workspace.clone()],
        shared_dirs: vec![],
    };
    std::fs::create_dir_all(&workspace).unwrap();
    SbxBackend
        .create(&CreateSpec {
            name: name.clone(),
            agent: "claude".into(),
            workspace: workspace.clone(),
            cpus: 2,
            memory: "2g".into(),
            skills: SkillsStore::Off,
            kits: vec![],
        })
        .unwrap();
    // The workspace is mounted at its host path in forward-slash form, drive
    // letter first and lowercase: E:\sbxm-it\x is /e/sbxm-it/x.
    let path = workspace.to_string_lossy().replace(char::from(92), "/");
    let in_sandbox = PathBuf::from(format!("/{}{}", path[..1].to_lowercase(), &path[2..]));
    let opts = HeadlessOpts {
        model: Some("claude-haiku-4-5-20251001".into()),
        high_effort: false,
        budget_usd: None,
        is_git_repo: false,
    };

    let result = headless::run(
        &SbxBackend,
        &name,
        &in_sandbox,
        Harness::Claude,
        "Reply with exactly: PONG",
        &opts,
        Duration::from_secs(120),
    )
    .unwrap();
    assert_eq!(result.status, RunStatus::Completed, "{result:?}");
    assert!(result.answer.contains("PONG"), "{result:?}");
    assert!(result.usage.output_tokens > 0 && result.usage.cost_usd.is_some());

    // A run that can't finish in 3 s is killed inside the sandbox: TimedOut,
    // and no `claude` process is left behind (decision 114).
    let result = headless::run(
        &SbxBackend,
        &name,
        &in_sandbox,
        Harness::Claude,
        "Run `sleep 60` in the shell, then reply DONE.",
        &opts,
        Duration::from_secs(3),
    )
    .unwrap();
    assert_eq!(result.status, RunStatus::TimedOut, "{result:?}");
    let left = SbxBackend
        .exec(
            &name,
            &ExecSpec {
                workdir: None,
                argv: vec!["pgrep".into(), "-x".into(), "claude".into()],
                stdin: Stdin::Closed,
            },
        )
        .unwrap();
    assert_eq!(left.exit_code, Some(1), "claude still running: {left:?}");
}

#[test]
#[ignore = "needs a logged-in sbx, the openai secret and SBXM_REAL_BASE_DIR"]
fn codex_headless_against_real_sbx() {
    use std::time::Duration;

    use sbxm::backend::{CreateSpec, ExecSpec, SkillsStore, Stdin};
    use sbxm::harness::Harness;
    use sbxm::headless::{self, HeadlessOpts, RunStatus};

    let base_dir = real_base_dir();
    let name = format!("sbxm-it-{}-codex-headless", std::process::id());
    let workspace = base_dir.join(&name);
    let _cleanup = Cleanup {
        sandbox: name.clone(),
        dirs: vec![workspace.clone()],
        shared_dirs: vec![],
    };
    std::fs::create_dir_all(&workspace).unwrap();
    SbxBackend
        .create(&CreateSpec {
            name: name.clone(),
            agent: "codex".into(),
            workspace: workspace.clone(),
            cpus: 2,
            memory: "2g".into(),
            skills: SkillsStore::Off,
            kits: vec![],
        })
        .unwrap();
    // E:\sbxm-it\x is mounted as /e/sbxm-it/x.
    let path = workspace.to_string_lossy().replace(char::from(92), "/");
    let in_sandbox = PathBuf::from(format!("/{}{}", path[..1].to_lowercase(), &path[2..]));
    // An unseeded workspace isn't a git repo (decision 113).
    let opts = HeadlessOpts {
        model: Some("gpt-5.6-luna".into()),
        high_effort: false,
        budget_usd: None,
        is_git_repo: false,
    };

    let result = headless::run(
        &SbxBackend,
        &name,
        &in_sandbox,
        Harness::Codex,
        "Reply with exactly: PONG",
        &opts,
        Duration::from_secs(180),
    )
    .unwrap();
    assert_eq!(result.status, RunStatus::Completed, "{result:?}");
    assert!(result.answer.contains("PONG"), "{result:?}");
    assert!(result.usage.output_tokens > 0 && result.usage.cost_usd.is_none());

    // Killed inside the sandbox after 3 s: TimedOut, and no `codex` process left.
    let result = headless::run(
        &SbxBackend,
        &name,
        &in_sandbox,
        Harness::Codex,
        "Run `sleep 60` in the shell, then reply DONE.",
        &opts,
        Duration::from_secs(3),
    )
    .unwrap();
    assert_eq!(result.status, RunStatus::TimedOut, "{result:?}");
    let left = SbxBackend
        .exec(
            &name,
            &ExecSpec {
                workdir: None,
                argv: vec!["pgrep".into(), "-x".into(), "codex".into()],
                stdin: Stdin::Closed,
            },
        )
        .unwrap();
    assert_eq!(left.exit_code, Some(1), "codex still running: {left:?}");
}

#[test]
#[ignore = "needs a logged-in sbx and SBXM_REAL_BASE_DIR"]
fn antigravity_sandbox_against_real_sbx() {
    use std::time::Duration;

    use sbxm::headless::{self, HeadlessOpts, RunStatus};

    let base_dir = real_base_dir();
    let project = format!("sbxm-it-{}-agy", std::process::id());
    let sandbox = format!("sbxm-{project}-antigravity");
    let _cleanup = Cleanup {
        sandbox: sandbox.clone(),
        dirs: vec![
            base_dir.join(&project),
            base_dir.join(".sbxm").join(&project),
        ],
        shared_dirs: vec![base_dir.join(".sbxm")],
    };
    let config_dir = TempDir::new().unwrap();
    let base = toml::Value::String(base_dir.to_str().unwrap().to_owned());
    std::fs::write(
        config_dir.path().join("config.toml"),
        format!("base_dir = {base}\n\n[resources]\ncpus = 2\nmemory = \"2g\"\n"),
    )
    .unwrap();
    let profile_dir = config_dir.path().join("profiles").join("default");
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join("profile.toml"),
        "[instructions]\nmandatory = \"mandatory.md\"\n\n[skills]\nstore = \"off\"\n",
    )
    .unwrap();
    std::fs::write(
        profile_dir.join("mandatory.md"),
        "REAL TEST CANARY: the word is QUINCE-7\n",
    )
    .unwrap();
    let agy = sbxm::harness::Harness::Antigravity;
    let options = new::Options {
        harness: agy,
        ..Default::default()
    };

    // The sandbox is created from the pinned kit, with the harness mixin.
    new::run(
        config_dir.path(),
        &project,
        &options,
        &SbxBackend,
        &mut std::io::stderr(),
    )
    .unwrap();

    let ls = Command::new("sbx").args(["ls", "--json"]).output().unwrap();
    let ls: serde_json::Value = serde_json::from_slice(&ls.stdout).unwrap();
    let entry = ls["sandboxes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == sandbox.as_str())
        .expect("sandbox listed");
    assert_eq!(entry["agent"], "antigravity");

    // Mandatory instructions land where `agy -p` loads them (P5).
    let agents_md = Command::new("sbx")
        .args(["exec", &sandbox, "cat", "/home/agent/.gemini/AGENTS.md"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&agents_md.stdout).trim(),
        "REAL TEST CANARY: the word is QUINCE-7"
    );

    // Nobody signed in to this fresh sandbox, so `agy -p` fails at auth. The
    // primitive reports that as `Failed`, not an error and not a timeout.
    let workspace = base_dir.join(&project);
    let path = workspace.to_string_lossy().replace(char::from(92), "/");
    let in_sandbox = PathBuf::from(format!("/{}{}", path[..1].to_lowercase(), &path[2..]));
    let result = headless::run(
        &SbxBackend,
        &sandbox,
        &in_sandbox,
        agy,
        "Reply with exactly: PONG",
        &HeadlessOpts {
            model: Some("gemini-3.8-flash-low".into()),
            high_effort: false,
            budget_usd: None,
            is_git_repo: false,
        },
        Duration::from_secs(60),
    )
    .unwrap();
    assert!(matches!(result.status, RunStatus::Failed(_)), "{result:?}");

    stop::run(config_dir.path(), &project, agy, &SbxBackend).unwrap();
    let rm_agy = rm::Options {
        harness: agy,
        ..Default::default()
    };
    rm::run(config_dir.path(), &project, &rm_agy, &SbxBackend, &Terminal).unwrap();
    let ls = Command::new("sbx").args(["ls", "--json"]).output().unwrap();
    assert!(!String::from_utf8_lossy(&ls.stdout).contains(&sandbox));
}

#[test]
#[ignore = "needs a logged-in sbx and SBXM_REAL_BASE_DIR"]
fn run_against_real_sbx() {
    use sbxm::commands::run;
    use sbxm::headless::RunStatus;

    // A base dir of its own, so the whole run vanishes with it.
    let base = real_base_dir().join(format!("sbxm-it-{}-run", std::process::id()));
    let _cleanup = Cleanup {
        sandbox: String::new(),
        dirs: vec![base.clone()],
        shared_dirs: vec![],
    };
    std::fs::create_dir_all(&base).unwrap();
    let config_dir = TempDir::new().unwrap();
    let base_toml = toml::Value::String(base.to_str().unwrap().to_owned());
    std::fs::write(
        config_dir.path().join("config.toml"),
        format!("base_dir = {base_toml}\n\n[resources]\ncpus = 2\nmemory = \"2g\"\n"),
    )
    .unwrap();
    let profile_dir = config_dir.path().join("profiles").join("default");
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join("profile.toml"),
        "description = \"real run test\"\n\n[skills]\nstore = \"off\"\n",
    )
    .unwrap();
    let run_config = config_dir.path().join("run.toml");
    std::fs::write(
        &run_config,
        "[task]\nprompt = \"Reply with exactly: PONG\"\n\n[run]\ntimeout = \"3m\"\n\n\
         [[contestants]]\nharness = \"claude\"\nmodel = \"claude-haiku-4-5-20251001\"\n\n\
         [[contestants]]\nharness = \"codex\"\nmodel = \"gpt-5.6-luna\"\n",
    )
    .unwrap();

    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let summary = run::run(
        config_dir.path(),
        &run_config,
        &SbxBackend,
        &mut out,
        &mut warn,
    )
    .unwrap();
    println!(
        "{}{}",
        String::from_utf8_lossy(&out),
        String::from_utf8_lossy(&warn)
    );

    // Both contestants ran in their own sandbox and answered.
    assert_eq!(summary.outcomes.len(), 2);
    for outcome in &summary.outcomes {
        let result = outcome.result.as_ref().unwrap();
        assert_eq!(result.status, RunStatus::Completed, "{result:?}");
        assert!(result.answer.contains("PONG"), "{result:?}");
        assert!(outcome.remove_error.is_none(), "{outcome:?}");
        // The workspace stays after the sandbox is gone.
        assert!(outcome.workspace.is_dir());
    }
    // The results are on disk: run.json is complete, each pair has its files.
    let meta = base.join(".sbxm").join("runs").join(&summary.run_id);
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(meta.join("run.json")).unwrap()).unwrap();
    assert_eq!(record["run_id"], summary.run_id.as_str());
    assert!(record["completed_at"].is_string(), "{record}");
    assert!(record["sbx_version"].is_string(), "{record}");
    assert_eq!(record["harnesses"].as_array().unwrap().len(), 2);
    for i in 0..2 {
        let dir = meta.join(i.to_string()).join("0");
        let result: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("result.json")).unwrap())
                .unwrap();
        assert_eq!(result["status"], "completed", "{result}");
        assert!(
            std::fs::read_to_string(dir.join("answer.md"))
                .unwrap()
                .contains("PONG")
        );
        assert!(
            !std::fs::read_to_string(dir.join("transcript.jsonl"))
                .unwrap()
                .is_empty()
        );
        assert!(dir.join("diff.patch").is_file());
    }
    assert!(meta.join("run-config.toml").is_file());
    // No run sandbox is left behind.
    let ls = Command::new("sbx").args(["ls", "--json"]).output().unwrap();
    let ls = String::from_utf8_lossy(&ls.stdout).into_owned();
    assert!(
        !ls.contains(&format!("sbxm-run-{}", summary.run_id)),
        "{ls}"
    );
}

#[test]
#[ignore = "needs a logged-in sbx and SBXM_REAL_BASE_DIR"]
fn run_diffs_against_real_sbx() {
    use sbxm::commands::run;
    use sbxm::headless::RunStatus;

    let base = real_base_dir().join(format!("sbxm-it-{}-diff", std::process::id()));
    let _cleanup = Cleanup {
        sandbox: String::new(),
        dirs: vec![base.clone()],
        shared_dirs: vec![],
    };
    std::fs::create_dir_all(&base).unwrap();
    let config_dir = TempDir::new().unwrap();
    let base_toml = toml::Value::String(base.to_str().unwrap().to_owned());
    std::fs::write(
        config_dir.path().join("config.toml"),
        format!("base_dir = {base_toml}\n\n[resources]\ncpus = 2\nmemory = \"2g\"\n"),
    )
    .unwrap();
    let profile_dir = config_dir.path().join("profiles").join("default");
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join("profile.toml"),
        "description = \"real diff test\"\n\n[skills]\nstore = \"off\"\n",
    )
    .unwrap();
    // A seed with history and a remote, to check it arrives as one clean commit.
    let seed = config_dir.path().join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    std::fs::write(seed.join("a.txt"), "alpha\n").unwrap();
    let seed_git = |args: &[&str]| {
        let ok = Command::new("git")
            .current_dir(&seed)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    };
    seed_git(&["init", "-q"]);
    seed_git(&["add", "-A"]);
    seed_git(&["commit", "-q", "-m", "old history"]);
    seed_git(&["remote", "add", "origin", "https://example.com/private.git"]);

    let contestants = "[[contestants]]\nharness = \"claude\"\nmodel = \"claude-haiku-4-5-20251001\"\n\n\
                       [[contestants]]\nharness = \"codex\"\nmodel = \"gpt-5.6-luna\"\n";
    let go = |name: &str, task: &str| {
        let run_config = config_dir.path().join(name);
        std::fs::write(
            &run_config,
            format!("{task}\n[run]\ntimeout = \"4m\"\n\n{contestants}"),
        )
        .unwrap();
        let (mut out, mut warn) = (Vec::new(), Vec::new());
        let summary = run::run(
            config_dir.path(),
            &run_config,
            &SbxBackend,
            &mut out,
            &mut warn,
        )
        .unwrap();
        println!(
            "{}{}",
            String::from_utf8_lossy(&out),
            String::from_utf8_lossy(&warn)
        );
        summary
    };

    // Seeded: each agent edits the seed's file and adds one.
    let toml_seed = toml::Value::String(seed.to_str().unwrap().to_owned());
    let seeded = go(
        "seeded.toml",
        &format!(
            "[task]\nprompt = \"Append the line 'edited by agent' to a.txt, and create hello.txt containing the word hi. Do not use git.\"\nseed = {toml_seed}\n"
        ),
    );
    for outcome in &seeded.outcomes {
        let result = outcome.result.as_ref().unwrap();
        assert_eq!(result.status, RunStatus::Completed, "{result:?}");
        let patch = &outcome.diff.as_ref().unwrap().as_ref().unwrap().patch;
        assert!(
            patch.contains("diff --git a/a.txt b/a.txt") && patch.contains("+edited by agent"),
            "{patch}"
        );
        assert!(
            patch.contains("diff --git a/hello.txt b/hello.txt"),
            "{patch}"
        );
        assert!(!patch.contains(".git/"), "{patch}");
        // One commit, no remote, in the workspace the agent had.
        let git_out = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(&outcome.workspace)
                .args(args)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        assert_eq!(git_out(&["remote"]), "");
    }

    // Unseeded: a plain folder, every new file in the diff, no `.git/` paths.
    let unseeded = go(
        "unseeded.toml",
        "[task]\nprompt = \"Create a file hello.txt containing the word hi. Do not use git.\"\n",
    );
    for outcome in &unseeded.outcomes {
        assert_eq!(
            outcome.result.as_ref().unwrap().status,
            RunStatus::Completed,
            "{outcome:?}"
        );
        let patch = &outcome.diff.as_ref().unwrap().as_ref().unwrap().patch;
        assert!(
            patch.contains("diff --git a/hello.txt b/hello.txt") && patch.contains("+hi"),
            "{patch}"
        );
        assert!(!outcome.workspace.join(".git").exists());
    }
    let ls = Command::new("sbx").args(["ls", "--json"]).output().unwrap();
    let ls = String::from_utf8_lossy(&ls.stdout).into_owned();
    assert!(!ls.contains("sbxm-run-"), "{ls}");
}

#[test]
#[ignore = "needs a logged-in sbx and SBXM_REAL_BASE_DIR"]
fn run_checks_against_real_sbx() {
    use sbxm::commands::run;
    use sbxm::headless::RunStatus;

    let base = real_base_dir().join(format!("sbxm-it-{}-checks", std::process::id()));
    let _cleanup = Cleanup {
        sandbox: String::new(),
        dirs: vec![base.clone()],
        shared_dirs: vec![],
    };
    std::fs::create_dir_all(&base).unwrap();
    let config_dir = TempDir::new().unwrap();
    let base_toml = toml::Value::String(base.to_str().unwrap().to_owned());
    std::fs::write(
        config_dir.path().join("config.toml"),
        format!("base_dir = {base_toml}\n\n[resources]\ncpus = 2\nmemory = \"2g\"\n"),
    )
    .unwrap();
    let profile_dir = config_dir.path().join("profiles").join("default");
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join("profile.toml"),
        "description = \"real checks test\"\n\n[skills]\nstore = \"off\"\n",
    )
    .unwrap();
    let seed = config_dir.path().join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    std::fs::write(seed.join("a.txt"), "alpha\n").unwrap();
    let seed_toml = toml::Value::String(seed.to_str().unwrap().to_owned());
    let run_config = config_dir.path().join("run.toml");
    std::fs::write(
        &run_config,
        format!(
            "[task]\nprompt = \"Create a file hello.txt containing the word hi. Do not use git.\"\nseed = {seed_toml}\n\n\
             [run]\ntimeout = \"4m\"\n\n\
             [[contestants]]\nharness = \"claude\"\nmodel = \"claude-haiku-4-5-20251001\"\n\n\
             [[contestants]]\nharness = \"codex\"\nmodel = \"gpt-5.6-luna\"\n\n\
             [[eval.checks]]\nid = \"has-hello\"\ncommand = \"test -f hello.txt && grep -q hi hello.txt\"\n\n\
             [[eval.checks]]\nid = \"seed-intact\"\ncommand = \"test -f a.txt\"\n\n\
             [[eval.checks]]\nid = \"deliberate-fail\"\ncommand = \"echo nope >&2; test -f does-not-exist.txt\"\n\n\
             [[eval.checks]]\nid = \"slow\"\ncommand = \"sleep 120\"\ntimeout = \"3s\"\n"
        ),
    )
    .unwrap();

    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let summary = run::run(
        config_dir.path(),
        &run_config,
        &SbxBackend,
        &mut out,
        &mut warn,
    )
    .unwrap();
    println!(
        "{}{}",
        String::from_utf8_lossy(&out),
        String::from_utf8_lossy(&warn)
    );

    let meta = base.join(".sbxm").join("runs").join(&summary.run_id);
    for outcome in &summary.outcomes {
        let result = outcome.result.as_ref().unwrap();
        assert_eq!(result.status, RunStatus::Completed, "{result:?}");
        // The checks ran in the sandbox and were judged by exit code.
        let verdicts: Vec<(&str, bool, bool)> = outcome
            .checks
            .iter()
            .map(|c| (c.id.as_str(), c.passed, c.timed_out))
            .collect();
        assert_eq!(
            verdicts,
            [
                ("has-hello", true, false),
                ("seed-intact", true, false),
                ("deliberate-fail", false, false),
                ("slow", false, true)
            ],
            "{:?}",
            outcome.checks
        );
        assert!(outcome.checks[2].output_tail.contains("nope"));
        // Saved next to the pair's other results, and the run finished normally.
        let evals: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                meta.join(outcome.contestant.to_string())
                    .join("0")
                    .join("evals.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(evals["checks"].as_array().unwrap().len(), 4);
        // The checks' side effects stay out of the diff.
        let patch = &outcome.diff.as_ref().unwrap().as_ref().unwrap().patch;
        assert!(patch.contains("hello.txt"), "{patch}");
    }
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(meta.join("run.json")).unwrap()).unwrap();
    assert!(record["completed_at"].is_string());
    let ls = Command::new("sbx").args(["ls", "--json"]).output().unwrap();
    let ls = String::from_utf8_lossy(&ls.stdout).into_owned();
    assert!(!ls.contains("sbxm-run-"), "{ls}");
}

#[test]
#[ignore = "needs a logged-in sbx and SBXM_REAL_BASE_DIR"]
fn run_judge_against_real_sbx() {
    use sbxm::commands::{run, run_show};
    use sbxm::headless::RunStatus;

    let base = real_base_dir().join(format!("sbxm-it-{}-judge", std::process::id()));
    let _cleanup = Cleanup {
        sandbox: String::new(),
        dirs: vec![base.clone()],
        shared_dirs: vec![],
    };
    std::fs::create_dir_all(&base).unwrap();
    let config_dir = TempDir::new().unwrap();
    let base_toml = toml::Value::String(base.to_str().unwrap().to_owned());
    std::fs::write(
        config_dir.path().join("config.toml"),
        format!("base_dir = {base_toml}\n\n[resources]\ncpus = 2\nmemory = \"2g\"\n"),
    )
    .unwrap();
    let profile_dir = config_dir.path().join("profiles").join("default");
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join("profile.toml"),
        "description = \"real judge test\"\n\n[skills]\nstore = \"off\"\n",
    )
    .unwrap();
    let run_config = config_dir.path().join("run.toml");
    std::fs::write(
        &run_config,
        "[task]\nprompt = \"In one sentence, explain what a mutex is. Reply with just that sentence.\"\n\n\
         [run]\ntimeout = \"4m\"\n\n\
         [[contestants]]\nharness = \"claude\"\nmodel = \"claude-haiku-4-5-20251001\"\n\n\
         [[contestants]]\nharness = \"codex\"\nmodel = \"gpt-5.6-luna\"\n\n\
         [[eval.rubric]]\nid = \"correct\"\nkind = \"pass_fail\"\nweight = 1.0\nnotes = \"Says a mutex gives one thread at a time exclusive access\"\n\n\
         [[eval.rubric]]\nid = \"clarity\"\nkind = \"scale\"\nlevels = [\"poor\", \"fair\", \"good\"]\nweight = 0.5\n\n\
         [eval.judge]\nharness = \"claude\"\nmodel = \"claude-haiku-4-5-20251001\"\n",
    )
    .unwrap();

    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let summary = run::run(
        config_dir.path(),
        &run_config,
        &SbxBackend,
        &mut out,
        &mut warn,
    )
    .unwrap();
    let (out, warn) = (
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&warn).into_owned(),
    );
    println!("{out}{warn}");

    for outcome in &summary.outcomes {
        assert_eq!(
            outcome.result.as_ref().unwrap().status,
            RunStatus::Completed,
            "{outcome:?}"
        );
    }
    // The judge ran in its own sandbox, once, and scored both contestants.
    assert!(
        out.contains("Judge claude/claude-haiku-4-5-20251001: repeat 1/1 scored 2 contestants"),
        "{out}"
    );
    let meta = base.join(".sbxm").join("runs").join(&summary.run_id);
    let read = |path: std::path::PathBuf| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    };
    let record = read(meta.join("judge").join("0").join("judge.json"));
    assert_eq!(record["status"], "ok", "{record}");
    let mut labels: Vec<String> = record["labels"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    labels.sort();
    assert_eq!(labels, ["A", "B"]);
    assert!(
        !std::fs::read_to_string(meta.join("judge").join("0").join("reply.txt"))
            .unwrap()
            .is_empty()
    );
    for contestant in 0..2 {
        let evals = read(
            meta.join(contestant.to_string())
                .join("0")
                .join("evals.json"),
        );
        let judge = &evals["judge"];
        assert_eq!(judge["status"], "ok", "{judge}");
        assert_eq!(judge["unscored"], serde_json::json!([]), "{judge}");
        assert!(
            judge["criteria"]["correct"]["value"].is_boolean(),
            "{judge}"
        );
        let clarity = judge["criteria"]["clarity"]["value"].as_str().unwrap();
        assert!(["poor", "fair", "good"].contains(&clarity), "{judge}");
        let score = judge["criteria"]["clarity"]["score"].as_f64().unwrap();
        assert!((0.0..=1.0).contains(&score));
    }
    // `run show` reveals which contestant was which candidate.
    let shown = run_show::render(
        config_dir.path(),
        &summary.run_id,
        &run_show::Options { full_diff: false },
    )
    .unwrap();
    assert!(
        shown.contains("Judge (candidate A):") && shown.contains("Judge (candidate B):"),
        "{shown}"
    );
    // The claude judge shares a provider with a claude contestant: warned, not refused.
    assert!(
        warn.contains("the judge (claude) uses the same provider (anthropic)"),
        "{warn}"
    );
    let ls = Command::new("sbx").args(["ls", "--json"]).output().unwrap();
    let ls = String::from_utf8_lossy(&ls.stdout).into_owned();
    assert!(!ls.contains("sbxm-run-"), "{ls}");
}

#[test]
#[ignore = "needs a logged-in sbx and SBXM_REAL_BASE_DIR"]
fn run_profiles_against_real_sbx() {
    use sbxm::commands::run;
    use sbxm::headless::RunStatus;

    let base = real_base_dir().join(format!("sbxm-it-{}-profiles", std::process::id()));
    let _cleanup = Cleanup {
        sandbox: String::new(),
        dirs: vec![base.clone()],
        shared_dirs: vec![],
    };
    std::fs::create_dir_all(&base).unwrap();
    let config_dir = TempDir::new().unwrap();
    let base_toml = toml::Value::String(base.to_str().unwrap().to_owned());
    std::fs::write(
        config_dir.path().join("config.toml"),
        format!("base_dir = {base_toml}\n\n[resources]\ncpus = 2\nmemory = \"2g\"\n"),
    )
    .unwrap();
    // Two profiles that differ in a visible setting: an environment variable.
    for (name, who) in [
        ("default", "from-the-default-profile"),
        ("strict", "from-the-strict-profile"),
    ] {
        let dir = config_dir.path().join("profiles").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("profile.toml"),
            format!("description = \"real profiles test\"\n\n[env]\nWHO = \"{who}\"\n\n[skills]\nstore = \"off\"\n"),
        )
        .unwrap();
    }
    let run_config = config_dir.path().join("run.toml");
    std::fs::write(
        &run_config,
        "[task]\nprompt = \"Run `printenv WHO` in the shell and reply with exactly its output and nothing else.\"\n\n\
         [run]\ntimeout = \"4m\"\n\n\
         [[contestants]]\nharness = \"claude\"\nmodel = \"claude-haiku-4-5-20251001\"\n\n\
         [[contestants]]\nharness = \"claude\"\nmodel = \"claude-haiku-4-5-20251001\"\nprofile = \"strict\"\n",
    )
    .unwrap();

    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let summary = run::run(
        config_dir.path(),
        &run_config,
        &SbxBackend,
        &mut out,
        &mut warn,
    )
    .unwrap();
    println!(
        "{}{}",
        String::from_utf8_lossy(&out),
        String::from_utf8_lossy(&warn)
    );

    // Each contestant's sandbox got its own profile's environment.
    let answers: Vec<String> = summary
        .outcomes
        .iter()
        .map(|o| {
            let result = o.result.as_ref().unwrap();
            assert_eq!(result.status, RunStatus::Completed, "{result:?}");
            result.answer.clone()
        })
        .collect();
    assert!(
        answers[0].contains("from-the-default-profile"),
        "{answers:?}"
    );
    assert!(
        answers[1].contains("from-the-strict-profile"),
        "{answers:?}"
    );
    // And the run recorded which profile each pair used.
    let meta = base.join(".sbxm").join("runs").join(&summary.run_id);
    let read = |path: std::path::PathBuf| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    };
    let record = read(meta.join("run.json"));
    assert_eq!(record["contestants"][0]["profile"], "default");
    assert_eq!(record["contestants"][1]["profile"], "strict");
    assert_eq!(record["harnesses"].as_array().unwrap().len(), 2);
    assert_eq!(
        read(meta.join("1").join("0").join("result.json"))["profile"],
        "strict"
    );
    let ls = Command::new("sbx").args(["ls", "--json"]).output().unwrap();
    assert!(!String::from_utf8_lossy(&ls.stdout).contains("sbxm-run-"));
}

#[test]
#[ignore = "needs a logged-in sbx and SBXM_REAL_BASE_DIR"]
fn run_kits_validate_against_real_sbx() {
    use sbxm::run::config::RunConfig;
    use sbxm::run::kits;

    let base_dir = real_base_dir();
    let run_dir = base_dir.join(format!("sbxm-it-{}-runkits", std::process::id()));
    let _cleanup = Cleanup {
        sandbox: String::new(),
        dirs: vec![run_dir.clone()],
        shared_dirs: vec![],
    };
    let config_dir = TempDir::new().unwrap();
    let base = toml::Value::String(base_dir.to_str().unwrap().to_owned());
    std::fs::write(
        config_dir.path().join("config.toml"),
        format!("base_dir = {base}\n\n[resources]\ncpus = 2\nmemory = \"2g\"\n"),
    )
    .unwrap();
    let profile_dir = config_dir.path().join("profiles").join("default");
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join("profile.toml"),
        "[network]\nallow = [\"example.org\"]\n\n[env]\nREAL_TEST = \"run-kit\"\n\n[instructions]\nmandatory = \"mandatory.md\"\n",
    )
    .unwrap();
    std::fs::write(profile_dir.join("mandatory.md"), "Real test canary.\n").unwrap();
    // Contestants: Claude and Codex; the judge is Antigravity, a harness no contestant uses.
    let run_config = config_dir.path().join("run.toml");
    std::fs::write(
        &run_config,
        "[task]\nprompt = \"p\"\n\n[run]\ncpus = 1\nmemory = \"1g\"\n\n\
         [[contestants]]\nharness = \"claude\"\nmodel = \"m\"\n\n\
         [[contestants]]\nharness = \"codex\"\nmodel = \"m\"\n\n\
         [[eval.rubric]]\nid = \"a\"\nkind = \"pass_fail\"\nweight = 1.0\n\n\
         [eval.judge]\nharness = \"antigravity\"\nmodel = \"m\"\n",
    )
    .unwrap();
    let run_config = RunConfig::load(&run_config).unwrap();

    let before = Command::new("sbx")
        .args(["ls", "--json"])
        .output()
        .unwrap()
        .stdout;
    let run = kits::build(
        config_dir.path(),
        &run_config,
        &run_dir.join("kits"),
        &SbxBackend,
    )
    .unwrap();

    // `sbx kit validate` accepted a common and a harness mixin for each of the three harnesses.
    assert_eq!(run.harnesses.len(), 3);
    for kit in &run.harnesses {
        assert_eq!(kit.dirs.len(), 2);
        assert!(kit.dirs.iter().all(|d| d.join("spec.yaml").is_file()));
    }
    assert_eq!(
        (run.resources.cpus, run.resources.memory.as_str()),
        (1, "1g")
    );
    // Validation only: no sandbox was created.
    let after = Command::new("sbx")
        .args(["ls", "--json"])
        .output()
        .unwrap()
        .stdout;
    assert_eq!(before, after);
}

/// A scratch world for `sbxm task` tests against a real `sbx`: a base dir on E:, a config with an
/// empty profile, a local bare repo standing in for GitHub, a target repo holding
/// `sbxm-task.toml` (and `files`), and a scripted GitHub with one open issue, #41. Everything is
/// removed when the test ends, sandboxes included.
struct RealTask {
    base_dir: PathBuf,
    config_dir: PathBuf,
    target: PathBuf,
    origin: PathBuf,
    github: sbxm::github::fake::FakeGitHub,
    _tmp: TempDir,
    _cleanups: Vec<Cleanup>,
}

fn real_git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn real_task_world(
    tag: &str,
    task_toml: &str,
    files: &[(&str, &str)],
    sandboxes: &[&str],
) -> RealTask {
    use sbxm::github::fake::FakeGitHub;
    use sbxm::github::{Issue, IssueText};

    let base_dir = real_base_dir().join(format!("sbxm-it-{}-{tag}", std::process::id()));
    let mut cleanups = vec![Cleanup {
        sandbox: String::new(),
        dirs: vec![base_dir.clone()],
        shared_dirs: vec![],
    }];
    cleanups.extend(sandboxes.iter().map(|name| Cleanup {
        sandbox: (*name).to_owned(),
        dirs: vec![],
        shared_dirs: vec![],
    }));
    std::fs::create_dir_all(&base_dir).unwrap();
    let tmp = TempDir::new().unwrap();

    // The config: base dir on E:, an empty profile.
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(config_dir.join("profiles").join("default")).unwrap();
    let base = toml::Value::String(base_dir.to_str().unwrap().to_owned());
    let profiles = toml::Value::String(config_dir.join("profiles").to_str().unwrap().to_owned());
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "base_dir = {base}\nprofiles_dir = {profiles}\ndefault_profile = \"default\"\n\n\
             [resources]\ncpus = 2\nmemory = \"2g\"\n"
        ),
    )
    .unwrap();
    std::fs::write(
        config_dir
            .join("profiles")
            .join("default")
            .join("profile.toml"),
        "description = \"real task test\"\n",
    )
    .unwrap();

    // GitHub's stand-in, and the target repo's config.
    let origin = tmp.path().join("origin.git");
    let seed = tmp.path().join("seed");
    std::fs::create_dir_all(&origin).unwrap();
    std::fs::create_dir_all(&seed).unwrap();
    real_git(&origin, &["init", "--bare", "-b", "main"]);
    real_git(&seed, &["init", "-b", "main"]);
    std::fs::write(seed.join("README.md"), "hello\n").unwrap();
    real_git(&seed, &["add", "-A"]);
    real_git(&seed, &["commit", "-m", "first"]);
    real_git(&seed, &["push", origin.to_str().unwrap(), "main"]);
    let target = tmp.path().join("target");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("sbxm-task.toml"), task_toml).unwrap();
    for (name, text) in files {
        std::fs::write(target.join(name), text).unwrap();
    }
    let github = FakeGitHub::default()
        .with_default_branch("main")
        .with_open_issues(vec![Issue {
            number: 41,
            title: "Add hello.txt".into(),
            labels: vec![],
            body: String::new(),
        }])
        .with_issue_text(IssueText {
            number: 41,
            title: "Add hello.txt".into(),
            state: "OPEN".into(),
            text: "title:\tAdd hello.txt\nstate:\tOPEN\n--\nAdd a hello.txt file that contains the word hi.\n".into(),
        });
    RealTask {
        base_dir,
        config_dir,
        target,
        origin,
        github,
        _tmp: tmp,
        _cleanups: cleanups,
    }
}

const REAL_WORKER_PROMPT: &str = "This is issue #{{number}}, branch {{branch}}. Do exactly this and nothing else:\n\
     1. Create a file hello.txt containing the word hi.\n\
     2. Run: git add hello.txt && git commit -m \"Add hello.txt. Fixes #{{number}}\"\n\
     3. Write .sbxm-task/result.md saying: hello.txt added.\n\
     4. Stop.\n";

/// Runs `sbxm task start --issue 41` for the world with a small real Claude worker.
fn real_task_start(world: &RealTask) {
    use sbxm::commands::task_start::{self, Options};
    use sbxm::task::record::SystemProbe;
    use sbxm::task::repo::Identity;

    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_start::run(
        &world.config_dir,
        &Options {
            repo_root: world.target.clone(),
            issues: vec![41],
            workers: None,
            worker_harness: None,
            worker_model: Some("claude-haiku-4-5-20251001".into()),
            time_limit: Some("5m".into()),
            profile: None,
            base: None,
            repo: Some("o/r".into()),
            clone_source: Some(world.origin.to_str().unwrap().to_owned()),
            identity: Some(Identity {
                name: "Dev".into(),
                email: "dev@example.com".into(),
            }),
        },
        &SbxBackend,
        &world.github,
        &SystemProbe,
        &mut out,
        &mut warn,
    );
    println!(
        "{}\n{}",
        String::from_utf8_lossy(&out),
        String::from_utf8_lossy(&warn)
    );
    result.unwrap();
}

/// `sbxm task start` with a real sandbox and a real (small) Claude worker, against a local
/// bare repo standing in for GitHub and a scripted GitHub: the worker's commit is collected
/// into the host-owned repo and its (empty) gates pass. Spends a few cents of Anthropic credit.
#[test]
#[ignore = "needs a logged-in sbx, the anthropic secret and SBXM_REAL_BASE_DIR; spends API credit"]
fn task_start_against_real_sbx() {
    use sbxm::task::record::{self, Stage, Status};
    use sbxm::task::repo;

    let world = real_task_world(
        "task",
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n\n[prompts]\nworker = \"worker.md\"\n",
        &[("worker.md", REAL_WORKER_PROMPT)],
        &["sbxm-task-issue-41-claude"],
    );

    real_task_start(&world);

    let meta = record::task_dir(&world.base_dir, "issue-41");
    let task = record::read(&meta.join("task.json")).unwrap();
    // The worker finished, and `start` gated it (there are no gates, so they pass).
    assert_eq!(
        (task.stage, task.status),
        (Stage::Gating, Status::Passed),
        "{task:?}"
    );
    let repo_git = meta.join("repo.git");
    assert!(repo::commits_ahead(&repo_git, "main", "issue-41").unwrap() >= 1);
    assert!(real_git(&repo_git, &["show", "issue-41:hello.txt"]).contains("hi"));
    assert!(meta.join("result.md").is_file(), "{task:?}");
    assert!(meta.join("transcripts").join("worker.jsonl").is_file());
    // The clone's files match what the sandbox's git expects: no line-ending noise.
    assert!(
        !task.notes.iter().any(|n| n.contains("uncommitted")),
        "{:?}",
        task.notes
    );
}

/// The whole path of `sbxm task start` then `sbxm task review` with a real sandbox, a real
/// (small) Claude worker, a real sandbox gate and a real Codex reviewer: the reviewer follows
/// the embedded prompt, writes `review.md` with its count line, and its sandbox and clone are
/// gone afterwards. Spends a little Anthropic and OpenAI credit.
#[test]
#[ignore = "needs a logged-in sbx, the anthropic and openai secrets and SBXM_REAL_BASE_DIR; spends API credit"]
fn task_review_against_real_sbx() {
    use sbxm::commands::task_review::{self, Options};
    use sbxm::task::gates::ShellHostRunner;
    use sbxm::task::record::{self, Stage, Status, SystemProbe};

    let world = real_task_world(
        "review",
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"test -f hello.txt\"]\n\n\
         [reviewer]\nharness = \"codex\"\n\n[prompts]\nworker = \"worker.md\"\n",
        &[("worker.md", REAL_WORKER_PROMPT)],
        &[
            "sbxm-task-issue-41-claude",
            "sbxm-task-issue-41-review-codex",
        ],
    );
    real_task_start(&world);

    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_review::run(
        &world.config_dir,
        &Options {
            repo_root: world.target.clone(),
            issue: 41,
            reviewer_harness: None,
            reviewer_model: None,
            reviewer_time_limit: Some("10m".into()),
            time_limit: Some("5m".into()),
            profile: None,
        },
        &SbxBackend,
        &SystemProbe,
        &ShellHostRunner,
        &mut out,
        &mut warn,
    );
    println!(
        "{}\n{}",
        String::from_utf8_lossy(&out),
        String::from_utf8_lossy(&warn)
    );
    result.unwrap();

    let meta = record::task_dir(&world.base_dir, "issue-41");
    let task = record::read(&meta.join("task.json")).unwrap();
    assert_eq!(
        (task.stage, task.status),
        (Stage::Ready, Status::Ok),
        "{task:?}"
    );
    let review = std::fs::read_to_string(meta.join("review.md")).unwrap();
    println!("{review}");
    assert!(review.starts_with("Reviewer: codex ("), "{review}");
    assert!(review.contains("Must-fix findings: "), "{review}");
    assert!(meta.join("review-1.md").is_file());
    assert!(meta.join("transcripts").join("review-1.jsonl").is_file());
    // The reviewer's clone is gone, and so is its sandbox.
    assert!(
        !world
            .base_dir
            .join("tasks")
            .join("issue-41-review")
            .exists()
    );
    let listing = Command::new("sbx").args(["ls", "--json"]).output().unwrap();
    assert!(
        !String::from_utf8_lossy(&listing.stdout).contains("sbxm-task-issue-41-review-codex"),
        "the reviewer's sandbox is still there"
    );
}

/// Sandbox gates in a real sandbox: each command runs as `sh -c` in the workspace under the
/// in-sandbox timeout, a failure stops the tier, and a command that outlives its limit is killed
/// there (nothing left running).
#[test]
#[ignore = "needs a logged-in sbx and SBXM_REAL_BASE_DIR"]
fn task_sandbox_gates_against_real_sbx() {
    use std::time::Duration;

    use sbxm::backend::{CreateSpec, ExecSpec, SkillsStore, Stdin};
    use sbxm::run::orchestrate::in_sandbox_path;
    use sbxm::task::gates::run_sandbox_tier;

    let base_dir = real_base_dir();
    let name = format!("sbxm-it-{}-gates", std::process::id());
    let workspace = base_dir.join(&name);
    let _cleanup = Cleanup {
        sandbox: name.clone(),
        dirs: vec![workspace.clone()],
        shared_dirs: vec![],
    };
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("marker.txt"), "here\n").unwrap();
    SbxBackend
        .create(&CreateSpec {
            name: name.clone(),
            agent: "claude".into(),
            workspace: workspace.clone(),
            cpus: 2,
            memory: "2g".into(),
            skills: SkillsStore::Off,
            kits: vec![],
        })
        .unwrap();
    let dir = in_sandbox_path(&workspace);
    let cmds = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();

    // Runs in the workspace; a failing command (exit 3) stops the tier.
    let outcomes = run_sandbox_tier(
        &SbxBackend,
        &name,
        &dir,
        "after-worker",
        &cmds(&["test -f marker.txt && echo found", "exit 3", "echo never"]),
        Duration::from_secs(60),
    );
    assert_eq!(outcomes.len(), 2, "{outcomes:?}");
    assert!(
        outcomes[0].result.passed && outcomes[0].output_tail.contains("found"),
        "{outcomes:?}"
    );
    assert_eq!(
        (outcomes[1].result.exit, outcomes[1].result.passed),
        (Some(3), false)
    );

    // A command that outlives its limit is killed inside the sandbox.
    let outcomes = run_sandbox_tier(
        &SbxBackend,
        &name,
        &dir,
        "after-worker",
        &cmds(&["sleep 60"]),
        Duration::from_secs(3),
    );
    assert!(
        outcomes[0].timed_out && !outcomes[0].result.passed,
        "{outcomes:?}"
    );
    let left = SbxBackend
        .exec(
            &name,
            &ExecSpec {
                workdir: None,
                // The sandbox has its own keep-alive `sleep`, so look for the gate's command line.
                argv: vec!["pgrep".into(), "-f".into(), "sleep 60".into()],
                stdin: Stdin::Closed,
            },
        )
        .unwrap();
    assert_eq!(
        left.exit_code,
        Some(1),
        "the gate's sleep is still running: {left:?}"
    );
}

/// Spike S10 as a test (decision 161): a bundle an agent makes in its sandbox is verified and
/// fetched into the host-owned repo, with the sandbox's own `git` and no host git in the workspace.
#[test]
#[ignore = "needs a logged-in sbx and SBXM_REAL_BASE_DIR"]
fn task_bundle_round_trip_against_real_sbx() {
    use sbxm::backend::{CreateSpec, ExecSpec, SkillsStore, Stdin};
    use sbxm::run::orchestrate::in_sandbox_path;
    use sbxm::task::repo::{self, BUNDLE_CAP, Identity};

    let base_dir = real_base_dir();
    let name = format!("sbxm-it-{}-bundle", std::process::id());
    let root = base_dir.join(&name);
    let _cleanup = Cleanup {
        sandbox: name.clone(),
        dirs: vec![root.clone()],
        shared_dirs: vec![],
    };
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };

    // A local bare repo plays GitHub; the task repo and the agent's clone come from it.
    let origin = root.join("origin.git");
    let seed = root.join("seed");
    std::fs::create_dir_all(&origin).unwrap();
    std::fs::create_dir_all(&seed).unwrap();
    git(&origin, &["init", "--bare", "-b", "main"]);
    git(&seed, &["init", "-b", "main"]);
    std::fs::write(seed.join("README.md"), "hello\n").unwrap();
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "-m", "first"]);
    git(&seed, &["push", origin.to_str().unwrap(), "main"]);
    let repo_git = root.join("task").join("repo.git");
    let workspace = root.join("task").join("workspace");
    repo::clone_bare(origin.to_str().unwrap(), &repo_git).unwrap();
    repo::create_branch(&repo_git, "issue-4", "main").unwrap();
    let identity = Identity {
        name: "Dev Person".into(),
        email: "dev@example.com".into(),
    };
    repo::clone_workspace(&repo_git, &workspace, "issue-4", &identity).unwrap();

    SbxBackend
        .create(&CreateSpec {
            name: name.clone(),
            agent: "claude".into(),
            workspace: workspace.clone(),
            cpus: 2,
            memory: "2g".into(),
            skills: SkillsStore::Off,
            kits: vec![],
        })
        .unwrap();

    // What the agent does, then the fixed bundle command (spec §6): all inside the sandbox.
    let sandbox_ws = in_sandbox_path(&workspace);
    let script = "echo agent > agent.txt && git add -A && git commit -q -m 'agent work' \
                  && mkdir -p .sbxm-task \
                  && git bundle create .sbxm-task/branch.bundle issue-4 ^origin/main";
    let out = SbxBackend
        .exec(
            &name,
            &ExecSpec {
                workdir: Some(sandbox_ws),
                argv: vec!["sh".into(), "-c".into(), script.into()],
                stdin: Stdin::Closed,
            },
        )
        .unwrap();
    assert_eq!(out.exit_code, Some(0), "sandbox git failed: {}", out.stderr);

    repo::fetch_bundle(&repo_git, &workspace, "issue-4", BUNDLE_CAP).unwrap();

    assert_eq!(
        repo::commits_ahead(&repo_git, "main", "issue-4").unwrap(),
        1
    );
}
