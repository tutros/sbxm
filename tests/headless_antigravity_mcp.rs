//! Issue 287: Antigravity's headless command removes `sbx`'s `mcp-gateway`
//! entry from `~/.gemini/config/mcp_config.json` in the same `sbx exec`, then
//! runs `agy`. These run the real argv with the host's `sh` and `jq` against a
//! fake `agy` that records its arguments; they skip where either is missing.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use sbxm::harness::Harness;
use sbxm::headless::HeadlessOpts;
use tempfile::TempDir;

const GATEWAY: &str = r#"{"type":"http","url":"http://mcp-gateway.docker.internal/mcp","headers":{"Authorization":"Bearer proxy-managed"}}"#;
const OTHER: &str = r#"{"command":"npx","args":["-y","some-server"],"env":{"A":"b"}}"#;

fn opts() -> HeadlessOpts {
    HeadlessOpts {
        model: Some("m".into()),
        high_effort: false,
        budget_usd: None,
        is_git_repo: false,
    }
}

/// Whether `sh` and `jq` can run here.
fn tools_available() -> bool {
    let runs = |tool: &str, args: &[&str]| {
        Command::new(tool)
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success())
    };
    runs("sh", &["-c", "true"]) && runs("jq", &["--version"])
}

struct Home {
    dir: TempDir,
}

impl Home {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let agy = bin.join("agy");
        // Records every argument, each followed by a NUL byte.
        std::fs::write(
            &agy,
            "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$HOME/agy-args\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&agy, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir }
    }

    fn config_dir(&self) -> PathBuf {
        self.dir.path().join(".gemini").join("config")
    }

    fn config(&self) -> PathBuf {
        self.config_dir().join("mcp_config.json")
    }

    fn write_config(&self, content: &str) {
        std::fs::create_dir_all(self.config_dir()).unwrap();
        std::fs::write(self.config(), content).unwrap();
    }

    /// Runs the Antigravity headless argv as `sbx exec` would, returning
    /// the arguments the fake `agy` received.
    fn run(&self, prompt: &str) -> Vec<String> {
        let argv = Harness::Antigravity.headless_argv(prompt, &opts()).unwrap();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![self.dir.path().join("bin")];
        paths.extend(std::env::split_paths(&path));
        let status = Command::new(&argv[0])
            .args(&argv[1..])
            .env("HOME", self.dir.path())
            .env("PATH", std::env::join_paths(paths).unwrap())
            .status()
            .unwrap();
        assert!(status.success(), "{status:?}");
        let recorded = std::fs::read(self.dir.path().join("agy-args")).expect("agy ran");
        let mut args: Vec<String> = recorded
            .split(|b| *b == 0)
            .map(|a| String::from_utf8(a.to_vec()).unwrap())
            .collect();
        assert_eq!(args.pop().as_deref(), Some(""), "trailing NUL");
        args
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(self.config()).unwrap()).unwrap()
    }

    /// The names in the config folder, so a leftover temporary file shows.
    fn config_entries(&self) -> Vec<String> {
        entries(&self.config_dir())
    }
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

macro_rules! require_tools {
    () => {
        if !tools_available() {
            eprintln!("skipped: sh or jq is not available");
            return;
        }
    };
}

#[test]
fn only_the_gateway_entry_leaves_an_empty_server_list() {
    require_tools!();
    let home = Home::new();
    home.write_config(&format!(r#"{{"mcpServers":{{"mcp-gateway":{GATEWAY}}}}}"#));
    let args = home.run("p");
    assert_eq!(args[0], "-p");
    assert_eq!(home.json(), serde_json::json!({"mcpServers": {}}));
    assert_eq!(home.config_entries(), ["mcp_config.json"]);
}

#[test]
fn another_server_is_kept_unchanged() {
    require_tools!();
    let home = Home::new();
    home.write_config(&format!(
        r#"{{"mcpServers":{{"mine":{OTHER},"mcp-gateway":{GATEWAY}}},"extra":1}}"#
    ));
    home.run("p");
    let other: serde_json::Value = serde_json::from_str(OTHER).unwrap();
    assert_eq!(
        home.json(),
        serde_json::json!({"mcpServers": {"mine": other}, "extra": 1})
    );
    assert_eq!(home.config_entries(), ["mcp_config.json"]);
}

#[test]
fn a_file_without_the_gateway_is_left_byte_for_byte() {
    require_tools!();
    let home = Home::new();
    let content = format!("{{ \"mcpServers\" : {{ \"mine\" : {OTHER} }} }}\n");
    home.write_config(&content);
    home.run("p");
    assert_eq!(std::fs::read_to_string(home.config()).unwrap(), content);
    assert_eq!(home.config_entries(), ["mcp_config.json"]);
}

#[test]
fn a_missing_file_is_not_created_and_agy_still_runs() {
    require_tools!();
    let home = Home::new();
    let args = home.run("p");
    assert_eq!(args[..2], ["-p", "p"]);
    assert!(!home.dir.path().join(".gemini").exists());
}

#[test]
fn a_missing_folder_entry_is_not_created_either() {
    require_tools!();
    let home = Home::new();
    std::fs::create_dir_all(home.config_dir()).unwrap();
    home.run("p");
    assert!(home.config_entries().is_empty());
}

#[test]
fn malformed_json_is_left_unchanged_and_agy_still_runs() {
    require_tools!();
    let home = Home::new();
    let content = r#"{"mcpServers":{"mcp-gateway":"#;
    home.write_config(content);
    let args = home.run("p");
    assert_eq!(args[..2], ["-p", "p"]);
    assert_eq!(std::fs::read_to_string(home.config()).unwrap(), content);
    assert_eq!(home.config_entries(), ["mcp_config.json"]);
}

#[test]
fn the_prompt_reaches_agy_byte_for_byte() {
    require_tools!();
    let home = Home::new();
    home.write_config(&format!(r#"{{"mcpServers":{{"mcp-gateway":{GATEWAY}}}}}"#));
    let prompt = "say \"hi\" and 'bye'; $HOME ${X} $(id) `id` \\ \n second line\n\ttab; exit 1";
    let args = home.run(prompt);
    assert_eq!(
        args,
        [
            "-p",
            prompt,
            "--model",
            "m",
            "--output-format",
            "stream-json",
            "--dangerously-skip-permissions",
        ]
    );
}

#[test]
fn other_harnesses_run_their_agent_directly() {
    let claude = Harness::Claude.headless_argv("p", &opts()).unwrap();
    assert_eq!(claude[..3], ["claude", "-p", "p"]);
    let codex = Harness::Codex.headless_argv("p", &opts()).unwrap();
    assert_eq!(codex[..2], ["codex", "exec"]);
    assert_eq!(codex.last().map(String::as_str), Some("p"));
}
