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

fn log(base: &Path, ids: &[&str], pid: Option<u32>) -> RunLog {
    RunLog::new(
        base,
        "# header".to_owned(),
        ids.iter().map(|s| (*s).to_owned()).collect(),
        pid,
        clock(),
        stage("working"),
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

fn record_of_pid(number: u32, pid: u32) -> Record {
    Record::new(
        &NewTask {
            kind: Kind::Issue,
            number,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
        },
        T0,
        Process::new(pid, T0),
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
    let mut log = log(base.path(), &[], Some(777));
    log.line("Starting issue-7 ...");

    record::write(&task_dir(base.path(), "issue-7"), &record_of_pid(7, 777)).unwrap();
    record::write(&task_dir(base.path(), "issue-8"), &record_of_pid(8, 999)).unwrap();
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
