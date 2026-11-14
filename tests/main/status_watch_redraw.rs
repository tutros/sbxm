//! `sbxm status --watch` timing and redraw (issue 577): the wait to the next draw is measured
//! from the start of a draw, the two `sbx` listings run in parallel, each section is written as
//! its read returns, and later `--watch` frames overwrite the previous one in place.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sbxm::backend::FakeBackend;
use sbxm::commands::status::{self, Env};
use sbxm::task::record::{self, Kind, NewTask, Process, ProcessProbe, Record, Stage};

const T0: u64 = 1_790_000_000;
const CLEAR: &str = "\x1B[2J\x1B[H";
const HOME: &str = "\x1B[H";
const EOL: &str = "\x1B[K";
const EOS: &str = "\x1B[J";

struct Probe;

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

fn disk() -> sbxm::status::footer::Disk {
    sbxm::status::footer::Disk {
        free_space: |_| Ok(40 * 1024 * 1024 * 1024),
        same_drive: |_, _| true,
        state_dir: Some(std::path::PathBuf::from("/state")),
        min_free_disk: sbxm::disk::DEFAULT_MIN_FREE,
    }
}

fn env(terminal: bool, iterations: Option<usize>) -> Env {
    Env {
        now: Box::new(|| T0),
        sleep: Box::new(|_| {}),
        terminal,
        width: 80,
        iterations,
        monotonic: Box::new(|| std::time::Duration::ZERO),
        slots: None,
        cloud_slots: None,
        disk: disk(),
    }
}

fn add_task(base: &Path, number: u32) {
    let mut task = Record::new(
        &NewTask {
            kind: Kind::Issue,
            number,
            repo: "o/r",
            title: "a title",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        T0,
        Process::new(1, T0),
    );
    task.advance(Stage::Working, T0, Process::new(1, T0))
        .unwrap();
    record::write(&record::task_dir(base, &task.id), &task).unwrap();
}

fn remove_tasks(base: &Path) {
    let _ = std::fs::remove_dir_all(base.join(".sbxm").join("tasks"));
}

/// A writer that records each `write` as one `write:<text>` event in a shared log.
struct LogWriter(Arc<Mutex<Vec<String>>>);

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        self.0.lock().unwrap().push(format!("write:{text}"));
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// ---- timing (acceptance 1, 2) -----------------------------------------------

/// The sleeps of a two-frame `--watch` at `interval` ms, when the local listing takes `local` ms
/// and the cloud one `cloud`, on a monotonic clock the listings advance. The whole-second epoch
/// clock `now` stays at `epoch` (a draw may cross a second boundary: `epoch_after`).
fn sleeps_ms(interval: u64, local: u64, cloud: u64, epoch: (u64, u64)) -> Vec<Duration> {
    let base = tempfile::tempdir().unwrap();
    let clock = Arc::new(AtomicU64::new(0));
    let hook_clock = Arc::clone(&clock);
    let backend = FakeBackend::default().with_listing_hook(move |which| {
        let took = if which == "local" { local } else { cloud };
        hook_clock.fetch_max(took, Ordering::SeqCst);
    });
    let slept = Arc::new(Mutex::new(Vec::new()));
    let (now_clock, mono_clock, sleep_log) =
        (Arc::clone(&clock), Arc::clone(&clock), Arc::clone(&slept));
    let mut env = env(true, Some(2));
    // The epoch seconds move with the same clock, from `epoch.0`, to `epoch.1` once it passes 0.
    env.now = Box::new(move || {
        if now_clock.load(Ordering::SeqCst) == 0 {
            epoch.0
        } else {
            epoch.1
        }
    });
    env.monotonic = Box::new(move || Duration::from_millis(mono_clock.load(Ordering::SeqCst)));
    env.sleep = Box::new(move |d| sleep_log.lock().unwrap().push(d));
    let mut out = Vec::new();
    status::run(
        base.path(),
        &backend,
        &Probe,
        &env,
        Some(Duration::from_millis(interval)),
        &mut out,
    )
    .unwrap();
    std::mem::take(&mut *slept.lock().unwrap())
}

fn sleeps(interval: u64, local: u64, cloud: u64) -> Vec<Duration> {
    sleeps_ms(interval * 1000, local * 1000, cloud * 1000, (T0, T0))
}

#[test]
fn the_sleep_is_the_interval_minus_the_time_the_draw_took() {
    // Parallel reads: the draw takes the longer of the two (4 s), not the sum (7 s).
    assert_eq!(sleeps(10, 4, 3), vec![Duration::from_secs(6)]);
    assert_eq!(sleeps(10, 3, 4), vec![Duration::from_secs(6)]);
}

#[test]
fn the_sleep_is_zero_when_the_draw_took_longer_than_the_interval() {
    assert_eq!(sleeps(2, 4, 3), vec![Duration::ZERO]);
}

#[test]
fn the_sleep_is_zero_when_the_draw_took_exactly_the_interval() {
    assert_eq!(sleeps(4, 4, 3), vec![Duration::ZERO]);
}

#[test]
fn a_draw_that_took_no_time_sleeps_the_whole_interval() {
    assert_eq!(sleeps(2, 0, 0), vec![Duration::from_secs(2)]);
}

#[test]
fn a_subsecond_draw_is_subtracted_from_the_interval() {
    // 400 ms at --interval 2 sleeps 1600 ms, whether or not the epoch second changed meanwhile.
    for epoch in [(T0, T0), (T0, T0 + 1)] {
        assert_eq!(
            sleeps_ms(2000, 400, 300, epoch),
            vec![Duration::from_millis(1600)],
            "{epoch:?}"
        );
    }
}

#[test]
fn fractional_draws_below_at_and_above_the_interval() {
    assert_eq!(
        sleeps_ms(2000, 1999, 0, (T0, T0)),
        vec![Duration::from_millis(1)]
    );
    assert_eq!(sleeps_ms(2000, 2000, 0, (T0, T0)), vec![Duration::ZERO]);
    assert_eq!(sleeps_ms(2000, 2001, 0, (T0, T0)), vec![Duration::ZERO]);
}

#[test]
fn the_two_listings_run_at_the_same_time() {
    let base = tempfile::tempdir().unwrap();
    let (to_local, from_cloud) = mpsc::channel::<()>();
    let (to_cloud, from_local) = mpsc::channel::<()>();
    let (to_local, to_cloud) = (Mutex::new(to_local), Mutex::new(to_cloud));
    let (from_local, from_cloud) = (Mutex::new(from_local), Mutex::new(from_cloud));
    let met = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&met);
    // Each listing announces itself, then waits (3 s at most) for the other to do the same: it
    // only does while both are in flight.
    let backend = FakeBackend::default().with_listing_hook(move |which| {
        let ok = if which == "cloud" {
            to_cloud.lock().unwrap().send(()).unwrap();
            from_cloud
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(3))
                .is_ok()
        } else {
            to_local.lock().unwrap().send(()).unwrap();
            from_local
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(3))
                .is_ok()
        };
        seen.lock().unwrap().push((which.to_owned(), ok));
    });
    let mut out = Vec::new();
    status::run(
        base.path(),
        &backend,
        &Probe,
        &env(false, None),
        None,
        &mut out,
    )
    .unwrap();
    let met = met.lock().unwrap().clone();
    assert_eq!(met.len(), 2, "{met:?}");
    assert!(met.iter().all(|(_, ok)| *ok), "{met:?}");
}

// ---- order of writes and calls (acceptance 3) -----------------------------------

#[test]
fn the_tasks_section_is_written_before_the_first_sbx_listing() {
    let base = tempfile::tempdir().unwrap();
    add_task(base.path(), 7);
    let log = Arc::new(Mutex::new(Vec::new()));
    let hook_log = Arc::clone(&log);
    let backend = FakeBackend::default()
        .with_listing_hook(move |which| hook_log.lock().unwrap().push(format!("call:{which}")));
    status::run(
        base.path(),
        &backend,
        &Probe,
        &env(false, None),
        None,
        &mut LogWriter(Arc::clone(&log)),
    )
    .unwrap();
    let log = log.lock().unwrap().clone();
    let tasks = log
        .iter()
        .position(|e| e.contains("issue-7"))
        .expect("tasks");
    let first_call = log
        .iter()
        .position(|e| e.starts_with("call:"))
        .expect("calls");
    assert!(tasks < first_call, "{log:?}");
    // The sections after it follow as their reads return, not all at the end.
    let sandboxes = log
        .iter()
        .position(|e| e.contains("No local sandboxes") || e.contains("SANDBOX"))
        .expect("sandboxes");
    assert!(first_call < sandboxes, "{log:?}");
}

#[test]
fn a_ready_cloud_section_is_written_while_the_local_listing_is_still_pending() {
    let base = tempfile::tempdir().unwrap();
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let hook_log = Arc::clone(&log);
    // The local listing waits (3 s at most) until the writer has received the cloud section.
    let backend = FakeBackend::default()
        .failing_cloud_list("cloud ready error")
        .with_listing_hook(move |which| {
            if which != "local" {
                return;
            }
            for _ in 0..300 {
                if hook_log
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|e| e.contains("cloud ready error"))
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let seen = hook_log
                .lock()
                .unwrap()
                .iter()
                .any(|e| e.contains("cloud ready error"));
            hook_log
                .lock()
                .unwrap()
                .push(format!("local done; cloud seen: {seen}"));
        });
    status::run(
        base.path(),
        &backend,
        &Probe,
        &env(false, None),
        None,
        &mut LogWriter(Arc::clone(&log)),
    )
    .unwrap();
    let log = log.lock().unwrap().clone();
    assert!(
        log.contains(&"local done; cloud seen: true".to_owned()),
        "{log:?}"
    );
    // The order of the sections is unchanged: tasks, cloud, then the local one.
    let at = |needle: &str| log.iter().position(|e| e.contains(needle)).expect(needle);
    assert!(at("No tasks") < at("cloud ready error") && at("cloud ready error") < at("local done"));
}

// ---- in-place redraw (acceptance 4, 5, 6) ------------------------------------------

/// The text of two `--watch` frames on a fake terminal: `between` changes the base dir after
/// the first.
fn two_frames(first: impl FnOnce(&Path), between: impl Fn(&Path) + 'static) -> (String, String) {
    let base = tempfile::tempdir().unwrap();
    first(base.path());
    let path = base.path().to_path_buf();
    let mut env = env(true, Some(2));
    env.sleep = Box::new(move |_| between(&path));
    let mut out = Vec::new();
    status::run(
        base.path(),
        &FakeBackend::default(),
        &Probe,
        &env,
        Some(Duration::from_secs(2)),
        &mut out,
    )
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.starts_with(CLEAR), "{text:?}");
    let split = text[CLEAR.len()..].find(HOME).expect("a second frame") + CLEAR.len();
    (text[..split].to_owned(), text[split..].to_owned())
}

fn check_in_place(first: &str, second: &str) {
    assert!(
        second.starts_with(HOME) && !second.starts_with(CLEAR),
        "{second:?}"
    );
    assert!(!first[CLEAR.len()..].contains(EOS), "{first:?}");
    assert!(first.starts_with(CLEAR));
    let body = second
        .strip_suffix(EOS)
        .expect("ends with erase-to-end-of-screen");
    let body = body.strip_prefix(HOME).unwrap();
    for line in body.split_inclusive('\n') {
        assert!(
            line.ends_with(&format!("{EOL}\n")),
            "{line:?} in {second:?}"
        );
    }
    assert!(body.ends_with('\n'), "{second:?}");
    assert!(body.matches(HOME).count() == 0, "{second:?}");
}

fn lines(frame: &str) -> usize {
    frame.matches('\n').count()
}

#[test]
fn the_first_frame_clears_and_the_second_overwrites_in_place() {
    let (first, second) = two_frames(|_| {}, |_| {});
    check_in_place(&first, &second);
    assert_eq!(first.matches('\n').count(), lines(&second));
    // The same content, line for line, apart from the codes.
    let plain = |s: &str| {
        s.replace(CLEAR, "")
            .replace(HOME, "")
            .replace(EOL, "")
            .replace(EOS, "")
    };
    assert_eq!(plain(&first), plain(&second));
}

#[test]
fn a_shorter_second_frame_ends_with_erase_to_end_of_screen() {
    let (first, second) = two_frames(|b| add_task(b, 7), remove_tasks);
    check_in_place(&first, &second);
    assert!(lines(&second) < lines(&first), "{first:?} {second:?}");
    assert!(first.contains("issue-7") && !second.contains("issue-7"));
}

#[test]
fn a_longer_second_frame_overwrites_every_line_of_the_first() {
    let (first, second) = two_frames(|_| {}, |b| add_task(b, 8));
    check_in_place(&first, &second);
    assert!(lines(&second) > lines(&first), "{first:?} {second:?}");
    assert!(second.contains("issue-8") && !first.contains("issue-8"));
}

#[test]
fn a_frame_of_the_same_length_with_other_content_is_overwritten() {
    let (first, second) = two_frames(
        |b| add_task(b, 7),
        |b| {
            remove_tasks(b);
            add_task(b, 9);
        },
    );
    check_in_place(&first, &second);
    assert_eq!(lines(&first), lines(&second));
    assert!(second.contains("issue-9") && !second.contains("issue-7"));
}

#[test]
fn not_a_terminal_draws_once_with_no_escape_codes() {
    let base = tempfile::tempdir().unwrap();
    let mut out = Vec::new();
    status::run(
        base.path(),
        &FakeBackend::default(),
        &Probe,
        &env(false, Some(3)),
        Some(Duration::from_secs(2)),
        &mut out,
    )
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(!text.contains('\x1B'), "{text:?}");
    assert!(!text.is_empty());
    let mut once = Vec::new();
    status::run(
        base.path(),
        &FakeBackend::default(),
        &Probe,
        &env(false, None),
        None,
        &mut once,
    )
    .unwrap();
    assert_eq!(text.as_bytes(), once.as_slice());
}

// ---- help (acceptance 7) ----------------------------------------------------------

#[test]
fn status_help_no_longer_says_it_never_calls_sbx() {
    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .args(["status", "--help"])
        .assert()
        .success()
        .get_output()
        .clone();
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(!help.contains("never calls `sbx`"), "{help}");
    assert!(help.contains("changes nothing"), "{help}");
    assert!(help.contains("through `sbx`"), "{help}");
}
