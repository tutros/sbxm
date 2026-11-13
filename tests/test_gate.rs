//! The `cargo test` gate in this repo's `sbxm-task.toml` (issue 619): it streams a line per test
//! binary and each failure line while `cargo test` runs, so a timeout keeps them, then prints the
//! summary and exits with `cargo test`'s own status. Each case runs the gate's own command with
//! `sh` (`dash` on Debian and Ubuntu, so no bash-only syntax passes) and a fake `cargo` on `PATH`.
//! Unix only: the gate runs in a Linux sandbox.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sbxm::task::config::TaskConfig;
use tempfile::TempDir;

fn test_gate() -> String {
    let config = TaskConfig::load(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
    let gates: Vec<_> = config
        .gates
        .sandbox
        .into_iter()
        .filter(|g| g.contains("cargo test"))
        .collect();
    assert_eq!(gates.len(), 1, "one cargo test gate: {gates:?}");
    gates.into_iter().next().unwrap()
}

struct Scratch {
    _tmp: TempDir,
    bin: PathBuf,
    home: PathBuf,
}

/// A fake `cargo` that runs `body` (a `sh` script) in place of `cargo test`.
fn scratch(body: &str) -> Scratch {
    let tmp = TempDir::new().unwrap();
    let bin = tmp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let cargo = bin.join("cargo");
    fs::write(&cargo, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    Scratch {
        _tmp: tmp,
        bin,
        home,
    }
}

impl Scratch {
    /// Runs the gate with `sh -c` under `program` (with its leading arguments, `timeout` for one).
    fn run_under(&self, program: &[&str]) -> Output {
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Command::new(program[0])
            .args(&program[1..])
            .arg("sh")
            .arg("-c")
            .arg(test_gate())
            .current_dir(&self.home)
            .env("PATH", path)
            .env("HOME", &self.home)
            .output()
            .unwrap()
    }

    fn run(&self) -> Output {
        self.run_under(&["env"])
    }

    fn log(&self) -> String {
        fs::read_to_string(self.home.join("t.log")).unwrap()
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// What cargo prints for a passing run, with the noise the gate leaves out.
const PASSING: &str = r#"echo "   Compiling sbxm v0.1.0" >&2
echo "    Finished test profile" >&2
echo "     Running unittests src/lib.rs (target/debug/deps/sbxm-1)" >&2
echo "running 2 tests"
echo "test noise_one ... ok"
echo "test noise_two ... ok"
echo "test result: ok. 2 passed"
echo "     Running tests/cli.rs (target/debug/deps/cli-2)" >&2
echo "test noise_three ... ok"
exit 0"#;

#[test]
fn a_passing_run_exits_zero_with_compact_output() {
    let s = scratch(PASSING);
    let out = s.run();
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("Running unittests src/lib.rs"), "{text}");
    assert!(text.contains("Running tests/cli.rs"), "{text}");
    assert!(!text.contains("noise"), "{text}");
    assert!(!text.contains("Compiling"), "{text}");
    // Two progress lines and the `df` line.
    assert_eq!(text.lines().count(), 3, "{text}");
    // The whole output still goes to the log.
    assert!(s.log().contains("test noise_three ... ok"), "{}", s.log());
    assert!(s.log().contains("Compiling sbxm"), "{}", s.log());
}

#[test]
fn a_failing_test_exits_with_cargo_tests_status_and_shows_the_failure() {
    let s = scratch(
        r#"echo "     Running tests/cli.rs (target/debug/deps/cli-2)" >&2
echo "test broken ... FAILED"
echo "thread 'broken' panicked at tests/cli.rs:3:5"
echo "only in the tail"
echo "test result: FAILED. 0 passed; 1 failed"
exit 101"#,
    );
    let out = s.run();
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(101), "{text}");
    assert!(text.contains("test broken ... FAILED"), "{text}");
    assert!(text.contains("panicked at tests/cli.rs"), "{text}");
    // The tail on failure, as before.
    assert!(text.contains("only in the tail"), "{text}");
}

#[test]
fn an_odd_exit_status_is_kept_not_the_pipes() {
    let s = scratch(
        r#"echo "     Running tests/cli.rs (target/debug/deps/cli-2)" >&2
exit 3"#,
    );
    let out = s.run();
    assert_eq!(out.status.code(), Some(3), "{}", stdout(&out));
}

// A full disk can stop the gate from recording `cargo test`'s status; that must not read as a pass.
#[test]
fn a_status_that_could_not_be_recorded_fails_the_gate() {
    let s = scratch(PASSING);
    fs::create_dir_all(s.home.join("t.rc")).unwrap();
    let out = s.run();
    assert_ne!(out.status.code(), Some(0), "{}", stdout(&out));
}

#[test]
fn a_compile_error_fails_and_shows_the_error_lines() {
    let s = scratch(
        r#"echo "   Compiling sbxm v0.1.0" >&2
echo "error[E0425]: cannot find value \`x\` in this scope" >&2
echo "error: could not compile \`sbxm\` (lib test) due to 1 previous error" >&2
exit 101"#,
    );
    let out = s.run();
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(101), "{text}");
    assert!(text.contains("error[E0425]: cannot find value"), "{text}");
    assert!(text.contains("error: could not compile"), "{text}");
}

#[test]
fn a_run_killed_by_the_timeout_keeps_the_lines_printed_so_far() {
    let s = scratch(
        r#"echo "     Running unittests src/lib.rs (target/debug/deps/sbxm-1)" >&2
echo "test noise_one ... ok"
echo "test early ... FAILED"
echo "     Running tests/cli.rs (target/debug/deps/cli-2)" >&2
sleep 30
exit 0"#,
    );
    let started = std::time::Instant::now();
    let out = s.run_under(&["timeout", "-k", "1", "2"]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(124), "{text}");
    assert!(started.elapsed().as_secs() < 20, "{text}");
    assert!(text.contains("Running unittests src/lib.rs"), "{text}");
    assert!(text.contains("Running tests/cli.rs"), "{text}");
    assert!(text.contains("test early ... FAILED"), "{text}");
    assert!(!text.contains("noise"), "{text}");
}

#[test]
fn the_gate_is_valid_posix_sh() {
    let out = Command::new("sh")
        .arg("-n")
        .arg("-c")
        .arg(test_gate())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    // No bash-only syntax that `dash -n` would still accept.
    let gate = test_gate();
    for bashism in ["pipefail", "PIPESTATUS", "[[", "|&", "&>"] {
        assert!(!gate.contains(bashism), "{bashism} in {gate}");
    }
}
