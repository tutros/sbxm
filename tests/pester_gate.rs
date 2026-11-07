//! The Pester gate in this repo's `sbxm-task.toml` (issue 455): it runs the `scripts/` tests only
//! when the task's change touches `scripts/`, and runs them when the base can't be found. Each case
//! runs the gate's own command with `sh` in a scratch clone, with a fake `just` that records the
//! call. Unix only: the gate runs in a Linux sandbox.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use sbxm::task::config::TaskConfig;
use tempfile::TempDir;

fn pester_gate() -> String {
    let config = TaskConfig::load(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
    let gates: Vec<_> = config
        .gates
        .sandbox
        .into_iter()
        .filter(|g| g.contains("script-test"))
        .collect();
    assert_eq!(gates.len(), 1, "one Pester gate: {gates:?}");
    gates.into_iter().next().unwrap()
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

fn write(dir: &Path, path: &str, text: &str) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

struct Scratch {
    _tmp: TempDir,
    work: PathBuf,
    bin: PathBuf,
    home: PathBuf,
}

/// A workspace like a task's: a clone (of a bare `origin` with `main` holding `scripts/a.ps1`,
/// `src/lib.rs` and `docs/readme.md`) on a new branch `task`. Without `origin`, a plain repo with
/// the same commit.
fn scratch(with_origin: bool) -> Scratch {
    let tmp = TempDir::new().unwrap();
    let seed = tmp.path().join("seed");
    fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q"]);
    write(&seed, "scripts/a.ps1", "a\n");
    write(&seed, "src/lib.rs", "lib\n");
    write(&seed, "docs/readme.md", "readme\n");
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "-qm", "base"]);
    let work = if with_origin {
        git(tmp.path(), &["clone", "-q", "--bare", "seed", "origin.git"]);
        git(tmp.path(), &["clone", "-q", "origin.git", "work"]);
        tmp.path().join("work")
    } else {
        seed
    };
    git(&work, &["checkout", "-qb", "task"]);
    let bin = tmp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let just = bin.join("just");
    fs::write(
        &just,
        "#!/bin/sh\necho \"$@\" > \"$(dirname \"$0\")/called\"\n",
    )
    .unwrap();
    fs::set_permissions(&just, fs::Permissions::from_mode(0o755)).unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    Scratch {
        _tmp: tmp,
        work,
        bin,
        home,
    }
}

impl Scratch {
    fn commit(&self, message: &str) {
        git(&self.work, &["add", "-A"]);
        git(&self.work, &["commit", "-qm", message]);
    }

    /// Runs the gate; returns whether it passed and whether it ran the Pester tests.
    fn run_gate(&self) -> (bool, bool) {
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let out = Command::new("sh")
            .arg("-c")
            .arg(pester_gate())
            .current_dir(&self.work)
            .env("PATH", path)
            .env("HOME", &self.home)
            .output()
            .unwrap();
        let called = fs::read_to_string(self.bin.join("called")).ok();
        if let Some(args) = &called {
            assert_eq!(args.trim(), "script-test");
        }
        (out.status.success(), called.is_some())
    }
}

#[test]
fn a_change_only_under_scripts_runs_the_tests() {
    let s = scratch(true);
    write(&s.work, "scripts/a.ps1", "changed\n");
    s.commit("scripts");
    assert_eq!(s.run_gate(), (true, true));
}

#[test]
fn a_change_under_scripts_and_src_runs_the_tests() {
    let s = scratch(true);
    write(&s.work, "scripts/b.ps1", "new\n");
    write(&s.work, "src/lib.rs", "changed\n");
    s.commit("both");
    assert_eq!(s.run_gate(), (true, true));
}

#[test]
fn a_change_outside_scripts_passes_without_running_them() {
    let s = scratch(true);
    write(&s.work, "src/lib.rs", "changed\n");
    s.commit("src");
    assert_eq!(s.run_gate(), (true, false));
}

#[test]
fn no_change_at_all_passes_without_running_them() {
    let s = scratch(true);
    assert_eq!(s.run_gate(), (true, false));
}

#[test]
fn deleting_a_script_runs_the_tests() {
    let s = scratch(true);
    fs::remove_file(s.work.join("scripts/a.ps1")).unwrap();
    s.commit("delete");
    assert_eq!(s.run_gate(), (true, true));
}

#[test]
fn moving_a_script_out_of_scripts_runs_the_tests() {
    let s = scratch(true);
    git(&s.work, &["mv", "scripts/a.ps1", "src/a.ps1"]);
    s.commit("move");
    assert_eq!(s.run_gate(), (true, true));
}

#[test]
fn a_file_named_like_scripts_elsewhere_does_not_run_them() {
    let s = scratch(true);
    write(&s.work, "docs/scripts.md", "doc\n");
    write(&s.work, "src/scripts/x.rs", "x\n");
    s.commit("lookalikes");
    assert_eq!(s.run_gate(), (true, false));
}

#[test]
fn an_uncommitted_change_under_scripts_runs_the_tests() {
    let s = scratch(true);
    write(&s.work, "scripts/a.ps1", "edited\n");
    write(&s.work, "scripts/new.ps1", "untracked\n");
    assert_eq!(s.run_gate(), (true, true));
}

#[test]
fn an_untracked_script_alone_runs_the_tests() {
    let s = scratch(true);
    write(&s.work, "scripts/new.ps1", "untracked\n");
    assert_eq!(s.run_gate(), (true, true));
}

// Git quotes a path with a non-ASCII character or a tab in its output (`core.quotePath`), so the
// gate must select `scripts/` by pathspec, never by matching the printed names (review M-1).
#[test]
fn a_committed_script_with_an_accented_name_runs_the_tests() {
    let s = scratch(true);
    write(&s.work, "scripts/café.ps1", "new\n");
    s.commit("accent");
    assert_eq!(s.run_gate(), (true, true));
}

#[test]
fn an_untracked_script_with_an_accented_name_runs_the_tests() {
    let s = scratch(true);
    write(&s.work, "scripts/café.ps1", "untracked\n");
    assert_eq!(s.run_gate(), (true, true));
}

#[test]
fn a_script_with_a_tab_in_its_name_runs_the_tests() {
    let s = scratch(true);
    write(&s.work, "scripts/a\tb.ps1", "new\n");
    s.commit("tab");
    assert_eq!(s.run_gate(), (true, true));
}

#[test]
fn a_missing_base_runs_the_tests() {
    let s = scratch(false);
    write(&s.work, "src/lib.rs", "changed\n");
    s.commit("src");
    assert_eq!(s.run_gate(), (true, true));
}

#[test]
fn a_failing_test_run_fails_the_gate() {
    let s = scratch(true);
    fs::write(
        s.bin.join("just"),
        "#!/bin/sh\necho \"$@\" > \"$(dirname \"$0\")/called\"; exit 1\n",
    )
    .unwrap();
    write(&s.work, "scripts/a.ps1", "changed\n");
    s.commit("scripts");
    let (passed, ran) = s.run_gate();
    assert!(ran);
    assert!(!passed);
}
