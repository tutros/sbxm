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
        format!("sandbox {sandbox} already exists; open it with `sbxm open {project}`")
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
