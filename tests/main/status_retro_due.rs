//! Issue 714 (decision 330(4)): `sbxm status` shows `retro due: N PRs since <end>` under the
//! loop's lines while the running loop's last retro check found a retro due, read from the line
//! the loop records in its lock.

use crate::common;

use std::fs;
use std::path::Path;

use common::Env as TestEnv;
use sbxm::backend::FakeBackend;
use sbxm::commands::status::{self, Env, LoopSource};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::queue::{LockFile, lock_path};
use sbxm::task::record::{Process, ProcessProbe};

const REPO: &str = "o/r";
const T0: u64 = 1_790_000_000;
const LOOP_START: u64 = T0 - 1_000;

struct Probe(Option<u64>);

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        self.0
    }
}

fn write_lock(base: &Path, retro_due: Option<&str>) {
    let path = lock_path(base);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = LockFile {
        process: Process::new(1, LOOP_START),
        issues: Vec::new(),
        cap: None,
        progress: None,
        adopted: Vec::new(),
        idle_check: None,
        took_over: Vec::new(),
        retro_due: retro_due.map(str::to_owned),
    };
    fs::write(path, serde_json::to_string(&file).unwrap()).unwrap();
}

fn env() -> Env {
    Env {
        placement: Default::default(),
        now: Box::new(|| T0),
        sleep: Box::new(|_| {}),
        terminal: false,
        width: 80,
        iterations: None,
        monotonic: Box::new(|| std::time::Duration::ZERO),
        slots: None,
        cloud_slots: None,
        disk: sbxm::status::footer::Disk {
            free_space: |_| Ok(40 * 1024 * 1024 * 1024),
            same_drive: |_, _| true,
            state_dir: Some(std::path::PathBuf::from("/state")),
            min_free_disk: sbxm::disk::DEFAULT_MIN_FREE,
        },
    }
}

/// One frame's Loop section, up to the disk footer.
fn loop_section(base: &Path, probe: &Probe) -> String {
    let gh = FakeGitHub::default();
    let source = LoopSource {
        github: &gh,
        repo: Ok(REPO.into()),
    };
    let mut out = Vec::new();
    status::run_with(
        base,
        &FakeBackend::default(),
        probe,
        &env(),
        None,
        Some(&source),
        &mut out,
    )
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    let section = text.split("\nLoop:").nth(1).expect("a Loop section");
    format!("Loop:{}", section.split("\nFree space:").next().unwrap())
}

#[test]
fn status_retro_due_a_running_loop_that_found_a_retro_due_shows_the_line() {
    let env = TestEnv::new();
    let base = env.base_dir();
    write_lock(&base, Some("retro due: 12 PRs since 2026-11-16"));

    let text = loop_section(&base, &Probe(Some(LOOP_START)));

    assert!(text.starts_with("Loop: running ("), "{text}");
    assert!(
        text.contains("\n  retro due: 12 PRs since 2026-11-16\n"),
        "{text}"
    );
}

#[test]
fn status_retro_due_no_line_when_the_loop_found_none_due() {
    let env = TestEnv::new();
    let base = env.base_dir();
    write_lock(&base, None);

    let text = loop_section(&base, &Probe(Some(LOOP_START)));

    assert!(text.starts_with("Loop: running ("), "{text}");
    assert!(!text.contains("retro due"), "{text}");
}

#[test]
fn status_retro_due_a_dead_loops_line_isnt_shown() {
    let env = TestEnv::new();
    let base = env.base_dir();
    write_lock(&base, Some("retro due: 12 PRs since 2026-11-16"));

    let text = loop_section(&base, &Probe(None));

    assert!(text.starts_with("Loop: not running"), "{text}");
    assert!(!text.contains("retro due"), "{text}");
}
