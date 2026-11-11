//! The disk footer of `sbxm status` (decision 171(b), issue 92): free space on the drive of
//! `base_dir` and on the drive of `sbx`'s sandbox state (the drives decision 194 checks), with a
//! warning under `min_free_disk`, read through a fake host as `doctor`'s tests do.

mod common;

use std::path::{Path, PathBuf};

use common::{Env as TestEnv, toml_path};
use sbxm::backend::FakeBackend;
use sbxm::commands::status::{self, Env, LoopSource};
use sbxm::config::GlobalConfig;
use sbxm::disk::{DEFAULT_MIN_FREE, describe};
use sbxm::github::fake::FakeGitHub;
use sbxm::status::footer::{self, Disk, Drive, Holds, Line};
use sbxm::task::record::ProcessProbe;

const GIB: u64 = 1024 * 1024 * 1024;
const T0: u64 = 1_790_000_000;

struct Probe;

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

fn base() -> PathBuf {
    PathBuf::from("/base")
}

fn state() -> PathBuf {
    PathBuf::from("/state")
}

/// `/base` has 40 GiB free, `/state` 10 GiB exactly, anything else can't be read; no two paths
/// share a drive.
fn disk(state_dir: Option<PathBuf>) -> Disk {
    Disk {
        free_space: |path: &Path| match path.to_str() {
            Some("/base") => Ok(40 * GIB),
            Some("/state") => Ok(DEFAULT_MIN_FREE),
            _ => Err(std::io::Error::other("no statvfs")),
        },
        same_drive: |a, b| a == b,
        state_dir,
        min_free_disk: DEFAULT_MIN_FREE,
    }
}

fn drive(dir: &str, holds: Holds, free: Result<u64, &str>) -> Line {
    Line::Drive(Drive {
        dir: PathBuf::from(dir),
        holds,
        free: free.map_err(str::to_owned),
    })
}

// ---- reading: which drives, through the fake host ------------------------------------------

#[test]
fn the_base_dir_and_the_state_folder_each_get_a_line() {
    assert_eq!(
        footer::read(&base(), &disk(Some(state()))),
        [
            drive("/base", Holds::Base, Ok(40 * GIB)),
            drive("/state", Holds::State, Ok(DEFAULT_MIN_FREE)),
        ]
    );
}

#[test]
fn the_same_drive_is_one_line() {
    let mut same = disk(Some(state()));
    same.same_drive = |_, _| true;

    assert_eq!(
        footer::read(&base(), &same),
        [drive("/base", Holds::Both, Ok(40 * GIB))]
    );
}

#[test]
fn a_drive_that_cannot_be_read_is_a_note_on_its_line() {
    assert_eq!(
        footer::read(&base(), &disk(Some(PathBuf::from("/gone")))),
        [
            drive("/base", Holds::Base, Ok(40 * GIB)),
            drive("/gone", Holds::State, Err("no statvfs")),
        ]
    );
}

#[test]
fn an_unknown_state_folder_says_so() {
    assert_eq!(
        footer::read(&base(), &disk(None)),
        [
            drive("/base", Holds::Base, Ok(40 * GIB)),
            Line::StateUnknown
        ]
    );
}

#[test]
fn the_real_disk_takes_sandbox_state_dir_and_min_free_disk_from_the_config() {
    let env = TestEnv::new();
    env.set_config(&format!(
        "min_free_disk = \"5g\"\nsandbox_state_dir = {}",
        toml_path(&state())
    ));
    let config = GlobalConfig::load(&env.config_dir()).unwrap();

    let real = Disk::real(&config);
    assert_eq!(real.state_dir, Some(state()));
    assert_eq!(real.min_free_disk, 5 * GIB);
}

// ---- rendering: the warning at the limit ---------------------------------------------------

#[test]
fn exactly_min_free_disk_is_no_warning_and_one_byte_under_is() {
    let at = [drive("/s", Holds::State, Ok(DEFAULT_MIN_FREE))];
    let under = [drive("/s", Holds::State, Ok(DEFAULT_MIN_FREE - 1))];

    assert_eq!(
        footer::render(&at, DEFAULT_MIN_FREE, None),
        "\nFree space: 10.0 GiB for sbx's sandbox state, on the drive of /s\n"
    );
    assert_eq!(
        footer::render(&under, DEFAULT_MIN_FREE, None),
        "\nFree space: warning, 10.0 GiB is under min_free_disk 10.0 GiB for sbx's sandbox \
         state, on the drive of /s\n"
    );
}

#[test]
fn min_free_disk_zero_never_warns() {
    let lines = [drive("/b", Holds::Base, Ok(0))];

    assert_eq!(
        footer::render(&lines, 0, None),
        "\nFree space: 0.0 GiB for base_dir, on the drive of /b\n"
    );
}

#[test]
fn every_kind_of_line() {
    let lines = [
        drive("/b", Holds::Both, Ok(40 * GIB)),
        drive("/g", Holds::State, Err("no statvfs")),
        Line::StateUnknown,
    ];

    assert_eq!(
        footer::render(&lines, DEFAULT_MIN_FREE, None),
        "\n\
         Free space: 40.0 GiB for base_dir and sbx's sandbox state, on the drive of /b\n\
         Free space: couldn't be read for sbx's sandbox state (no statvfs), on the drive of /g\n\
         Free space: sbx's sandbox state folder isn't known on this platform; set \
         sandbox_state_dir in config.toml to show its drive\n"
    );
}

#[test]
fn a_terminal_cuts_each_line_to_its_width() {
    let lines = [drive("/b", Holds::Both, Ok(GIB))];

    let text = footer::render(&lines, DEFAULT_MIN_FREE, Some(30));
    for line in text.lines() {
        let line: &str = line;
        assert!(line.chars().count() <= 30, "{line:?}");
    }
}

// ---- rendering: a terminal keeps the verdict (review M-2) ---------------------------------

/// A path long enough that the verdict after it would be cut at 80 columns.
const LONG: &str = "/home/someone/projects/sandboxes/sbxm-base-dir-with-a-rather-long-name";

#[test]
fn one_byte_under_still_warns_on_an_eighty_column_terminal_with_a_long_path() {
    let under = [drive(LONG, Holds::Both, Ok(DEFAULT_MIN_FREE - 1))];
    let at = [drive(LONG, Holds::Both, Ok(DEFAULT_MIN_FREE))];

    let warned = footer::render(&under, DEFAULT_MIN_FREE, Some(80));
    let line = warned.lines().nth(1).unwrap();
    assert!(line.chars().count() <= 80, "{line:?}");
    assert!(line.contains("warning"), "{line:?}");
    assert!(line.contains("under min_free_disk 10.0 GiB"), "{line:?}");
    let fine = footer::render(&at, DEFAULT_MIN_FREE, Some(80));
    assert!(!fine.contains("warning"), "{fine:?}");
    let never = footer::render(&under, 0, Some(80));
    assert!(!never.contains("warning"), "{never:?}");
}

#[test]
fn an_unreadable_drive_says_so_on_an_eighty_column_terminal_with_a_long_path() {
    let lines = [drive(LONG, Holds::Both, Err("no statvfs"))];

    let text = footer::render(&lines, DEFAULT_MIN_FREE, Some(80));
    let line = text.lines().nth(1).unwrap();
    assert!(line.chars().count() <= 80, "{line:?}");
    assert!(line.contains("couldn't be read"), "{line:?}");
}

// ---- the command: the footer comes last ----------------------------------------------------

fn env(disk: Disk) -> Env {
    Env {
        now: Box::new(|| T0),
        sleep: Box::new(|_| {}),
        terminal: false,
        width: 80,
        iterations: None,
        monotonic: Box::new(|| std::time::Duration::ZERO),
        slots: None,
        disk,
    }
}

#[test]
fn status_ends_with_the_footer_after_the_loop_section() {
    let base = tempfile::tempdir().unwrap();
    let gh = FakeGitHub::default();
    let source = LoopSource {
        github: &gh,
        repo: Ok("o/r".into()),
    };
    let mut out = Vec::new();

    status::run_with(
        base.path(),
        &FakeBackend::default(),
        &Probe,
        &env(disk(Some(state()))),
        None,
        Some(&source),
        &mut out,
    )
    .unwrap();

    let text = String::from_utf8(out).unwrap();
    // A real temp dir: on Windows it has a drive prefix, so name it as the footer does.
    let footer = format!(
        "\nFree space: couldn't be read for base_dir (no statvfs), on {}\n\
         Free space: 10.0 GiB for sbx's sandbox state, on the drive of /state\n",
        describe(base.path())
    );
    assert!(text.ends_with(&footer), "{text}");
    let loop_at = text.find("\nLoop").expect(&text);
    assert!(loop_at < text.len() - footer.len(), "{text}");
}

#[test]
fn a_drive_that_cannot_be_read_never_fails_status() {
    let mut out = Vec::new();

    status::run(
        Path::new("/nonexistent-base"),
        &FakeBackend::default(),
        &Probe,
        &env(disk(Some(PathBuf::from("/gone")))),
        None,
        &mut out,
    )
    .unwrap();

    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains(
            "couldn't be read for sbx's sandbox state (no statvfs), on the drive of /gone"
        ),
        "{text}"
    );
}

#[test]
fn the_cli_warns_under_min_free_disk_on_the_state_drive() {
    let env = TestEnv::new();
    let state = env.tmp.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    // No real drive has a thousand TiB free.
    env.set_config(&format!(
        "min_free_disk = \"1000t\"\nsandbox_state_dir = {}",
        toml_path(&state)
    ));

    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["status"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("for base_dir"), "{stdout}");
    assert!(
        stdout.contains("is under min_free_disk 1024000.0 GiB for "),
        "{stdout}"
    );
}
