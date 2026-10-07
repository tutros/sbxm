//! Decision 170, part 1 (issue 87): `run.log` in the task folder: a copy of what a task command
//! prints, each line with a timestamp and the stage, a header per invocation, no secret values,
//! gone with the task. The clock and the stage lookup are faked.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use sbxm::task::record::{self, Kind, NewTask, Process, Record};
use sbxm::task::runlog::{RunLog, Tee};

const T0: u64 = 1_790_000_000; // 2026-09-21T14:13:20Z

fn clock() -> Box<dyn Fn() -> u64 + Send> {
    let now = Arc::new(AtomicU64::new(T0));
    Box::new(move || now.fetch_add(1, Ordering::SeqCst))
}

fn stage(name: &'static str) -> Box<dyn Fn(&Path) -> String + Send> {
    Box::new(move |_| name.to_owned())
}

fn log(base: &Path, ids: &[&str], identity: Option<Process>) -> RunLog {
    RunLog::new(
        base,
        "# header".to_owned(),
        ids.iter().map(|s| (*s).to_owned()).collect(),
        identity,
        clock(),
        stage("working"),
        Box::new(|_| {}),
    )
}

fn task_dir(base: &Path, id: &str) -> std::path::PathBuf {
    let dir = record::task_dir(base, id);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn read(base: &Path, id: &str) -> String {
    fs::read_to_string(record::task_dir(base, id).join("run.log")).unwrap()
}

fn record_of_process(number: u32, process: Process) -> Record {
    Record::new(
        &NewTask {
            kind: Kind::Issue,
            number,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        T0,
        process,
    )
}

#[test]
fn a_log_starts_with_the_header_then_one_stamped_line_per_line() {
    let base = tempfile::tempdir().unwrap();
    task_dir(base.path(), "issue-5");
    let mut log = log(base.path(), &["issue-5"], None);

    log.line("Starting issue-5 ...");
    log.line("issue-5: worker done");

    assert_eq!(
        read(base.path(), "issue-5"),
        "# header\n\
         2026-09-21T14:13:20Z [working] Starting issue-5 ...\n\
         2026-09-21T14:13:21Z [working] issue-5: worker done\n"
    );
}

#[test]
fn lines_printed_before_the_task_folder_exists_are_written_when_it_appears() {
    let base = tempfile::tempdir().unwrap();
    let mut log = log(base.path(), &["issue-5"], None);
    log.line("#3: skipped, a question");
    assert!(!record::task_dir(base.path(), "issue-5").exists());

    task_dir(base.path(), "issue-5");
    log.line("Starting issue-5 ...");

    assert_eq!(
        read(base.path(), "issue-5"),
        "# header\n\
         2026-09-21T14:13:20Z [-] #3: skipped, a question\n\
         2026-09-21T14:13:21Z [working] Starting issue-5 ...\n"
    );
}

#[test]
fn a_removed_task_folder_is_not_made_again_by_the_log() {
    let base = tempfile::tempdir().unwrap();
    let dir = task_dir(base.path(), "issue-5");
    let mut log = log(base.path(), &["issue-5"], None);
    log.line("removing");
    fs::remove_dir_all(&dir).unwrap();

    log.line("removed sandbox x");

    assert!(!dir.exists());
}

#[test]
fn a_task_folder_made_again_gets_a_fresh_header() {
    let base = tempfile::tempdir().unwrap();
    let dir = task_dir(base.path(), "issue-5");
    let mut log = log(base.path(), &["issue-5"], None);
    log.line("before");
    fs::remove_dir_all(&dir).unwrap();
    task_dir(base.path(), "issue-5");

    log.line("after");

    let text = read(base.path(), "issue-5");
    assert!(text.starts_with("# header\n"), "{text}");
    assert!(text.contains("[working] after"), "{text}");
}

#[test]
fn secret_looking_values_never_reach_the_log() {
    let base = tempfile::tempdir().unwrap();
    task_dir(base.path(), "issue-5");
    let mut log = log(base.path(), &["issue-5"], None);

    log.line("token sk-ant-api03-AbCdEf0123456789xyz and ghp_0123456789abcdefABCDEF01 and AIzaSyA0123456789abcdefghij");
    log.line("github_pat_11ABCDEFG0123456789_abcdefghijklmnop");

    let text = read(base.path(), "issue-5");
    for secret in [
        "sk-ant-api03-AbCdEf0123456789xyz",
        "ghp_0123456789abcdefABCDEF01",
        "AIzaSyA0123456789abcdefghij",
        "github_pat_11ABCDEFG0123456789_abcdefghijklmnop",
    ] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    assert!(
        text.contains("token [redacted] and [redacted] and [redacted]"),
        "{text}"
    );
}

#[test]
fn a_task_made_by_this_process_gets_the_log_without_being_named() {
    let base = tempfile::tempdir().unwrap();
    let mut log = log(base.path(), &[], Some(Process::new(777, T0)));
    log.line("Starting issue-7 ...");

    record::write(
        &task_dir(base.path(), "issue-7"),
        &record_of_process(7, Process::new(777, T0)),
    )
    .unwrap();
    record::write(
        &task_dir(base.path(), "issue-8"),
        &record_of_process(8, Process::new(999, T0)),
    )
    .unwrap();
    log.line("issue-7: worker done");

    let text = read(base.path(), "issue-7");
    assert!(text.contains("[-] Starting issue-7 ..."), "{text}");
    assert!(text.contains("[working] issue-7: worker done"), "{text}");
    // Another process's task is not ours.
    assert!(
        !record::task_dir(base.path(), "issue-8")
            .join("run.log")
            .exists()
    );
}

/// Decision 163, issue 129 M-3: a pid alone isn't the process; its start time must match too, or
/// a reused pid would make a stale task look like this process's and receive its whole output.
#[test]
fn a_record_with_the_current_pid_but_a_different_start_time_is_not_treated_as_this_process() {
    let base = tempfile::tempdir().unwrap();
    let mut log = log(base.path(), &[], Some(Process::new(777, T0)));
    log.line("Starting issue-9 ...");

    record::write(
        &task_dir(base.path(), "issue-9"),
        &record_of_process(9, Process::new(777, T0 + 1000)),
    )
    .unwrap();
    log.line("issue-9: worker done");

    assert!(
        !record::task_dir(base.path(), "issue-9")
            .join("run.log")
            .exists(),
        "a stale record with a reused pid must not receive this process's output"
    );
}

/// Both of a command's writers (screen output and warnings) feed one log.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn the_tee_passes_every_byte_on_unchanged_and_logs_whole_lines() {
    let base = tempfile::tempdir().unwrap();
    task_dir(base.path(), "issue-5");
    let log = Arc::new(Mutex::new(log(base.path(), &["issue-5"], None)));
    let screen = Sink::default();
    let warnings = Sink::default();
    {
        let mut out = Tee::new(screen.clone(), Arc::clone(&log));
        let mut warn = Tee::new(warnings.clone(), Arc::clone(&log));
        out.write_all(b"one ").unwrap();
        out.write_all(b"line\r\nsecond\n").unwrap();
        warn.write_all(b"warning: x\n").unwrap();
        out.write_all(b"no newline at the end").unwrap();
    }

    assert_eq!(
        screen.0.lock().unwrap().as_slice(),
        b"one line\r\nsecond\nno newline at the end"
    );
    assert_eq!(warnings.0.lock().unwrap().as_slice(), b"warning: x\n");
    let text = read(base.path(), "issue-5");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "# header");
    assert!(lines[1].ends_with("[working] one line"), "{text}");
    assert!(lines[2].ends_with("[working] second"), "{text}");
    assert!(lines[3].ends_with("[working] warning: x"), "{text}");
    assert!(
        lines[4].ends_with("[working] no newline at the end"),
        "{text}"
    );
}

#[test]
fn the_header_names_the_command_the_exe_its_hash_the_commit_and_the_pid() {
    let text = sbxm::task::runlog::header(
        T0,
        &["sbxm".to_owned(), "task".to_owned(), "start".to_owned()],
        Path::new("/bin/sbxm"),
        "abc123",
        Some("415da41"),
        4242,
    );
    assert_eq!(
        text,
        "# 2026-09-21T14:13:20Z sbxm task start | exe /bin/sbxm | sha256 abc123 | commit 415da41 | pid 4242"
    );
    let unknown = sbxm::task::runlog::header(T0, &[], Path::new("x"), "h", None, 1);
    assert!(unknown.contains("| commit unknown |"), "{unknown}");
}

#[test]
fn a_secret_on_the_command_line_is_masked_in_the_header() {
    let text = sbxm::task::runlog::header(
        T0,
        &[
            "sbxm".to_owned(),
            "--token".to_owned(),
            "ghp_0123456789abcdefABCDEF01".to_owned(),
        ],
        Path::new("x"),
        "h",
        None,
        1,
    );
    assert!(!text.contains("ghp_0123456789abcdefABCDEF01"), "{text}");
    assert!(text.contains("--token [redacted]"), "{text}");
}

/// Issue 129, M-4: a log that can't be appended to (a deterministic failure, not just "the task
/// folder doesn't exist yet") still lets the command run, but warns once, not once per line, so
/// the missing provenance isn't silent.
#[test]
fn an_append_failure_warns_once_naming_the_path_and_cause_not_once_per_line() {
    let base = tempfile::tempdir().unwrap();
    let dir = task_dir(base.path(), "issue-5");
    // `run.log` is a directory, so opening it for append fails deterministically.
    fs::create_dir_all(dir.join("run.log")).unwrap();
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&warnings);
    let mut log = RunLog::new(
        base.path(),
        "# header".to_owned(),
        vec!["issue-5".to_owned()],
        None,
        clock(),
        stage("working"),
        Box::new(move |msg: &str| sink.lock().unwrap().push(msg.to_owned())),
    );

    log.line("one");
    log.line("two");

    let warned = warnings.lock().unwrap();
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert!(
        warned[0].contains(&dir.join("run.log").display().to_string()),
        "{warned:?}"
    );
}

/// A command with no task to log into (`task init`, `file-findings --file`) must stay silent:
/// there is no failure, just nothing to write to.
#[test]
fn no_target_and_no_identity_never_warns() {
    let base = tempfile::tempdir().unwrap();
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&warnings);
    let mut log = RunLog::new(
        base.path(),
        "# header".to_owned(),
        Vec::new(),
        None,
        clock(),
        stage("working"),
        Box::new(move |msg: &str| sink.lock().unwrap().push(msg.to_owned())),
    );

    log.line("nothing to log");

    assert!(warnings.lock().unwrap().is_empty());
}
