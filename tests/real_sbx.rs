//! Tests against the real `sbx`. Run with:
//! `SBXM_REAL_BASE_DIR=<dir outside %TEMP%/AppData> cargo test --test real_sbx -- --ignored`

use std::path::PathBuf;
use std::process::{Command, Stdio};

use sbxm::backend::SbxBackend;
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

/// The HTTP status a request from inside the sandbox gets. The egress proxy
/// answers blocked hosts with 403, reported by curl as the CONNECT status.
/// `printenv <name>` inside the sandbox, trimmed.
fn printenv(sandbox: &str, name: &str) -> String {
    let output = Command::new("sbx")
        .args(["exec", sandbox, "printenv", name])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

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
        .expect("set SBXM_REAL_BASE_DIR to an existing dir outside %TEMP%/AppData");
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
        "description = \"real sbx test\"\n\n[network]\nallow = [\"example.org\"]\n\n[env]\nREAL_TEST_GREETING = \"hello from the profile\"\n",
    )
    .unwrap();

    new::run(
        config_dir.path(),
        &project,
        &new::Options::default(),
        &SbxBackend,
    )
    .unwrap();

    let ls = Command::new("sbx").args(["ls", "--json"]).output().unwrap();
    let ls = String::from_utf8(ls.stdout).unwrap();
    assert!(
        ls.contains(&format!("\"{sandbox}\"")),
        "sbx ls --json: {ls}"
    );

    // Slice 10b: the profile's allow list reaches the sandbox; other hosts stay blocked.
    assert_eq!(http_status(&sandbox, "https://example.org"), "200");
    assert_eq!(http_status(&sandbox, "https://example.com"), "403");

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

    // Slice 5: `list` sees it through the real `sbx ls --json`, joined with state.
    let entries = list::entries(config_dir.path(), &SbxBackend).unwrap();
    let entry = entries
        .iter()
        .find(|e| e.sandbox == sandbox)
        .unwrap_or_else(|| panic!("{sandbox} not in {entries:?}"));
    assert_eq!(entry.project, project);
    assert_eq!(entry.harness, "claude");
    assert_eq!(entry.problem, None);
    println!("list status for {sandbox}: {}", entry.status);

    // Slice 6: `stop` stops it, and `list` shows that.
    stop::run(config_dir.path(), &project, &SbxBackend).unwrap();
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
