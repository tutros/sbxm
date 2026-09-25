//! Tests against the real `sbx`. Run with:
//! `SBXM_REAL_BASE_DIR=<dir outside %TEMP%/AppData> cargo test --test real_sbx -- --ignored`

use std::path::PathBuf;
use std::process::Command;

use sbxm::backend::SbxBackend;
use sbxm::commands::{list, new};
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
        let _ = Command::new("sbx")
            .args(["rm", "-f", &self.sandbox])
            .status();
        for dir in &self.dirs {
            let _ = std::fs::remove_dir_all(dir);
        }
        for dir in &self.shared_dirs {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

fn real_base_dir() -> PathBuf {
    let dir = std::env::var_os("SBXM_REAL_BASE_DIR")
        .expect("set SBXM_REAL_BASE_DIR to an existing dir outside %TEMP%/AppData");
    PathBuf::from(dir)
}

#[test]
#[ignore = "needs a logged-in sbx and SBXM_REAL_BASE_DIR"]
fn new_creates_a_real_sandbox() {
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
}
