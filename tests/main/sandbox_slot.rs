//! The machine-wide sandbox slots (issue 504, decision 304): taking one, waiting in line for one,
//! `--no-wait`, and taking over a gone holder's.

use crate::common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::Env;
use sbxm::config::GlobalConfig;
use sbxm::slot::{self, Gate, Holder, Registry};
use sbxm::task::record::{Process, ProcessProbe};

/// This process started at 1000; the pids in `others` are alive with their start times.
struct Probe {
    others: Vec<(u32, u64)>,
}

impl ProcessProbe for Probe {
    fn start_time(&self, pid: u32) -> Option<u64> {
        if pid == std::process::id() {
            return Some(1000);
        }
        self.others
            .iter()
            .find(|(other, _)| *other == pid)
            .map(|(_, started)| *started)
    }
}

const A: u32 = 4_100_001;
const B: u32 = 4_100_002;
const DEAD: u32 = 4_100_009;

fn config(env: &Env, slots: Option<u32>) -> GlobalConfig {
    if let Some(n) = slots {
        let path = env.config_dir().join("config.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("{text}sandbox_slots = {n}\n")).unwrap();
    }
    GlobalConfig::load(&env.config_dir()).unwrap()
}

fn slots_dir(env: &Env) -> PathBuf {
    env.tmp.path().join("local-data").join("sbxm").join("slots")
}

type Sleeps = Arc<Mutex<Vec<Duration>>>;

/// A gate over `env`'s own slots folder; `on_sleep` runs at each wait, given how many came before.
fn gate(
    env: &Env,
    others: Vec<(u32, u64)>,
    on_sleep: impl Fn(usize) + Send + Sync + 'static,
) -> (Gate, Sleeps) {
    let sleeps: Sleeps = Arc::default();
    let seen = Arc::clone(&sleeps);
    let gate = Gate {
        dir: Some(slots_dir(env)),
        no_wait: false,
        probe: Arc::new(Probe { others }),
        sleep: Arc::new(move |pause| {
            let n = {
                let mut seen = seen.lock().unwrap();
                seen.push(pause);
                seen.len() - 1
            };
            assert!(n < 20, "waited too long");
            on_sleep(n);
        }),
        command: "sbxm task review --pr 9".to_owned(),
        catch_ctrl_c: false,
        registry: Arc::new(Registry::default()),
        stop: None,
    };
    (gate, sleeps)
}

fn holder(pid: u32, started: u64, command: &str, task: Option<&str>) -> Holder {
    Holder {
        process: Process::new(pid, started),
        token: format!("t{pid}"),
        command: command.to_owned(),
        task: task.map(str::to_owned),
        since: "2026-11-08T10:00:00Z".to_owned(),
    }
}

fn write(path: &Path, holder: &Holder) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string(holder).unwrap()).unwrap();
}

fn slot_file(dir: &Path, k: u32) -> PathBuf {
    dir.join(format!("slot-{k}.json"))
}

fn read_holder(path: &Path) -> Holder {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn queue(dir: &Path) -> Vec<String> {
    match std::fs::read_dir(dir.join("queue")) {
        Ok(entries) => entries
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn a_holder() -> Holder {
    holder(A, 2000, "sbxm task run --issue 7", Some("issue-7"))
}

const A_LINE: &str = "sbxm task run --issue 7 (task issue-7, process 4100001, since \
                      2026-11-08T10:00:00Z)";

#[test]
fn unset_takes_nothing_and_writes_no_file() {
    let env = Env::new();
    let config = config(&env, None);
    let (gate, sleeps) = gate(&env, vec![], |_| panic!("no wait without a limit"));
    let mut warn = Vec::new();
    let first = slot::take(&config, &gate, Some("issue-1"), &mut warn).unwrap();
    let second = std::thread::scope(|s| {
        s.spawn(|| slot::take(&config, &gate, Some("issue-2"), &mut Vec::new()).unwrap())
            .join()
            .unwrap()
    });
    assert_eq!(first.path(), None);
    assert_eq!(second.path(), None);
    assert!(!env.tmp.path().join("local-data").exists());
    assert!(sleeps.lock().unwrap().is_empty());
    assert!(warn.is_empty());
}

#[test]
fn a_free_slot_is_taken_at_once_naming_the_command_and_task_and_given_back_on_drop() {
    let env = Env::new();
    let config = config(&env, Some(1));
    let (gate, sleeps) = gate(&env, vec![], |_| panic!("a free slot doesn't wait"));
    let mut warn = Vec::new();
    let slot = slot::take(&config, &gate, Some("issue-3"), &mut warn).unwrap();
    let dir = slots_dir(&env);
    let path = slot_file(&dir, 1);
    assert_eq!(slot.path(), Some(path.as_path()));
    let held = read_holder(&path);
    assert_eq!(held.process.pid, std::process::id());
    assert_eq!(held.command, "sbxm task review --pr 9");
    assert_eq!(held.task.as_deref(), Some("issue-3"));
    assert!(queue(&dir).is_empty(), "{:?}", queue(&dir));
    assert!(warn.is_empty());
    assert!(sleeps.lock().unwrap().is_empty());
    drop(slot);
    assert!(!path.exists());
}

#[test]
fn a_command_already_holding_the_slot_never_waits_on_itself() {
    let env = Env::new();
    let config = config(&env, Some(1));
    let (gate, _) = gate(&env, vec![], |_| panic!("waited on itself"));
    let outer = slot::take(&config, &gate, Some("issue-4"), &mut Vec::new()).unwrap();
    // `task run` going on to its review: the review's own take is the same slot.
    let inner = slot::take(&config, &gate, Some("issue-4"), &mut Vec::new()).unwrap();
    assert_eq!(inner.path(), outer.path());
    drop(inner);
    let path = slot_file(&slots_dir(&env), 1);
    assert!(path.exists(), "the outer command still holds it");
    drop(outer);
    assert!(!path.exists());
}

#[test]
fn a_second_command_waits_printing_the_holder_once_and_starts_after_the_release() {
    let env = Env::new();
    let config = config(&env, Some(1));
    let dir = slots_dir(&env);
    write(&slot_file(&dir, 1), &a_holder());
    let released = slot_file(&dir, 1);
    let (gate, sleeps) = gate(&env, vec![(A, 2000)], move |n| {
        if n == 2 {
            std::fs::remove_file(&released).unwrap();
        }
    });
    let mut warn = Vec::new();
    let slot = slot::take(&config, &gate, None, &mut warn).unwrap();
    assert_eq!(
        String::from_utf8(warn).unwrap(),
        format!("waiting for a sandbox slot: held by {A_LINE}\n")
    );
    // Looked at every 5 s, and taken at the first look after the release.
    assert_eq!(*sleeps.lock().unwrap(), vec![slot::POLL; 3]);
    assert_eq!(slot::POLL, Duration::from_secs(5));
    assert_eq!(
        read_holder(slot.path().unwrap()).process.pid,
        std::process::id()
    );
    assert!(queue(&dir).is_empty(), "its ticket is gone");
}

#[test]
fn the_holder_is_printed_again_when_it_changes() {
    let env = Env::new();
    let config = config(&env, Some(1));
    let dir = slots_dir(&env);
    write(&slot_file(&dir, 1), &a_holder());
    let path = slot_file(&dir, 1);
    let (gate, _) = gate(&env, vec![(A, 2000), (B, 3000)], move |n| match n {
        0 => write(&path, &holder(B, 3000, "sbxm template build rust", None)),
        1 => std::fs::remove_file(&path).unwrap(),
        _ => {}
    });
    let mut warn = Vec::new();
    let _slot = slot::take(&config, &gate, None, &mut warn).unwrap();
    assert_eq!(
        String::from_utf8(warn).unwrap(),
        format!(
            "waiting for a sandbox slot: held by {A_LINE}\nwaiting for a sandbox slot: held by \
             sbxm template build rust (process 4100002, since 2026-11-08T10:00:00Z)\n"
        )
    );
}

#[test]
fn two_waiting_commands_start_in_arrival_order() {
    let env = Env::new();
    let config = config(&env, Some(1));
    let dir = slots_dir(&env);
    write(&slot_file(&dir, 1), &a_holder());
    // B came first: its ticket sorts before any made now.
    let b_ticket = dir
        .join("queue")
        .join(format!("{:032}-{B:010}-000000.json", 1_000_000_000u64));
    let b = holder(B, 3000, "sbxm run eval.toml", None);
    write(&b_ticket, &b);
    let (slot_path, ticket) = (slot_file(&dir, 1), b_ticket.clone());
    let (gate, sleeps) = gate(&env, vec![(A, 2000), (B, 3000)], move |n| match n {
        // A ends: the slot is free, but B is in line first.
        0 => std::fs::remove_file(&slot_path).unwrap(),
        // B takes it.
        1 => {
            assert!(!slot_path.exists(), "the later command took B's turn");
            write(&slot_path, &b);
            std::fs::remove_file(&ticket).unwrap();
        }
        // B ends.
        2 => std::fs::remove_file(&slot_path).unwrap(),
        _ => {}
    });
    let mut warn = Vec::new();
    let slot = slot::take(&config, &gate, None, &mut warn).unwrap();
    assert_eq!(sleeps.lock().unwrap().len(), 3);
    assert_eq!(
        read_holder(slot.path().unwrap()).process.pid,
        std::process::id()
    );
    let b_line = "sbxm run eval.toml (process 4100002, since 2026-11-08T10:00:00Z)";
    assert_eq!(
        String::from_utf8(warn).unwrap(),
        format!(
            "waiting for a sandbox slot: held by {A_LINE}\nwaiting for a sandbox slot: {b_line} \
             is waiting ahead of it\nwaiting for a sandbox slot: held by {b_line}\n"
        )
    );
}

/// `task loop` takes the slot per task: a command started by hand while the loop's task holds it
/// gets it when that task ends, and the loop's next task waits behind it.
#[test]
fn a_hand_command_runs_between_two_loop_tasks_and_the_next_one_waits() {
    let env = Env::new();
    let config = config(&env, Some(1));
    let dir = slots_dir(&env);
    let (first, _) = gate(&env, vec![(B, 3000)], |_| panic!("the first task waited"));
    let task_1 = slot::take(&config, &first, Some("issue-1"), &mut Vec::new()).unwrap();
    // The hand command, started meanwhile, waits in line.
    let hand = dir
        .join("queue")
        .join(format!("{:032}-{B:010}-000000.json", 1u64));
    let b = holder(B, 3000, "sbxm task review --pr 5", Some("pr-5"));
    write(&hand, &b);
    drop(task_1);

    let (slot_path, ticket) = (slot_file(&dir, 1), hand.clone());
    let (second, sleeps) = gate(&env, vec![(B, 3000)], move |n| match n {
        // The hand command takes the free slot first.
        0 => {
            assert!(!slot_path.exists(), "the loop's next task jumped the line");
            write(&slot_path, &b);
            std::fs::remove_file(&ticket).unwrap();
        }
        // ... and ends.
        1 => std::fs::remove_file(&slot_path).unwrap(),
        _ => {}
    });
    let mut warn = Vec::new();
    let task_2 = slot::take(&config, &second, Some("issue-2"), &mut warn).unwrap();
    assert_eq!(sleeps.lock().unwrap().len(), 2);
    assert_eq!(
        read_holder(task_2.path().unwrap()).task.as_deref(),
        Some("issue-2")
    );
    let line = "sbxm task review --pr 5 (task pr-5, process 4100002, since 2026-11-08T10:00:00Z)";
    assert_eq!(
        String::from_utf8(warn).unwrap(),
        format!(
            "waiting for a sandbox slot: {line} is waiting ahead of it\nwaiting for a sandbox \
             slot: held by {line}\n"
        )
    );
}

#[test]
fn two_slots_let_two_commands_run_and_a_third_waits() {
    let env = Env::new();
    let config = config(&env, Some(2));
    let dir = slots_dir(&env);
    write(&slot_file(&dir, 1), &a_holder());
    let (gate2, _) = gate(&env, vec![(A, 2000)], |_| panic!("slot 2 was free"));
    let second = slot::take(&config, &gate2, None, &mut Vec::new()).unwrap();
    assert_eq!(second.path(), Some(slot_file(&dir, 2).as_path()));

    let third = std::thread::scope(|s| {
        s.spawn(|| {
            let path = slot_file(&dir, 1);
            let (gate3, sleeps) = gate(&env, vec![(A, 2000)], move |_| {
                std::fs::remove_file(&path).unwrap();
            });
            let mut warn = Vec::new();
            let slot = slot::take(&config, &gate3, None, &mut warn).unwrap();
            assert_eq!(sleeps.lock().unwrap().len(), 1);
            assert_eq!(
                String::from_utf8(warn).unwrap(),
                format!("waiting for a sandbox slot: held by {A_LINE}\n")
            );
            slot.path().unwrap().to_owned()
        })
        .join()
        .unwrap()
    });
    assert_eq!(third, slot_file(&dir, 1));
}

#[test]
fn no_wait_with_no_free_slot_refuses_naming_the_holder_and_writes_nothing() {
    let env = Env::new();
    let config = config(&env, Some(1));
    let dir = slots_dir(&env);
    let a = a_holder();
    write(&slot_file(&dir, 1), &a);
    let (mut gate, sleeps) = gate(&env, vec![(A, 2000)], |_| panic!("--no-wait waited"));
    gate.no_wait = true;
    let err = format!(
        "{:#}",
        slot::take(&config, &gate, None, &mut Vec::new()).unwrap_err()
    );
    assert_eq!(
        err,
        format!(
            "no free sandbox slot (sandbox_slots = 1 in config.toml): held by {A_LINE}; run this \
             command again once it ends, or without --no-wait to wait for a slot"
        )
    );
    assert!(sleeps.lock().unwrap().is_empty());
    assert!(queue(&dir).is_empty(), "{:?}", queue(&dir));
    assert_eq!(read_holder(&slot_file(&dir, 1)), a);
}

#[test]
fn no_wait_with_a_free_slot_takes_it() {
    let env = Env::new();
    let config = config(&env, Some(1));
    let (mut gate, _) = gate(&env, vec![], |_| panic!("--no-wait waited"));
    gate.no_wait = true;
    let slot = slot::take(&config, &gate, None, &mut Vec::new()).unwrap();
    assert_eq!(slot.path(), Some(slot_file(&slots_dir(&env), 1).as_path()));
}

#[test]
fn no_wait_does_not_jump_ahead_of_a_waiting_command() {
    let env = Env::new();
    let config = config(&env, Some(1));
    let dir = slots_dir(&env);
    write(
        &dir.join("queue")
            .join(format!("{:032}-{B:010}-000000.json", 1u64)),
        &holder(B, 3000, "sbxm run eval.toml", None),
    );
    let (mut gate, _) = gate(&env, vec![(B, 3000)], |_| panic!("--no-wait waited"));
    gate.no_wait = true;
    let err = format!(
        "{:#}",
        slot::take(&config, &gate, None, &mut Vec::new()).unwrap_err()
    );
    assert!(err.contains("sbxm run eval.toml"), "{err}");
    assert!(err.contains("is waiting ahead of it"), "{err}");
    assert!(!slot_file(&dir, 1).exists());
}

#[test]
fn no_wait_takes_a_free_slot_the_older_waiters_do_not_need() {
    let env = Env::new();
    let config = config(&env, Some(2));
    let dir = slots_dir(&env);
    let ticket = dir
        .join("queue")
        .join(format!("{:032}-{B:010}-000000.json", 1u64));
    write(&ticket, &holder(B, 3000, "sbxm run eval.toml", None));
    let (mut gate, _) = gate(&env, vec![(B, 3000)], |_| panic!("--no-wait waited"));
    gate.no_wait = true;

    let slot = slot::take(&config, &gate, None, &mut Vec::new()).unwrap();

    assert_eq!(slot.path(), Some(slot_file(&dir, 1).as_path()));
    assert!(
        !slot_file(&dir, 2).exists(),
        "the older waiter's slot stays free"
    );
    assert!(ticket.exists(), "the older waiter is still in line");
    assert_eq!(queue(&dir).len(), 1, "--no-wait writes no ticket");
}

#[test]
fn no_wait_refuses_when_the_older_waiters_need_every_free_slot() {
    let env = Env::new();
    let config = config(&env, Some(2));
    let dir = slots_dir(&env);
    write(&slot_file(&dir, 1), &a_holder());
    write(
        &dir.join("queue")
            .join(format!("{:032}-{B:010}-000000.json", 1u64)),
        &holder(B, 3000, "sbxm run eval.toml", None),
    );
    let (mut gate, _) = gate(&env, vec![(A, 2000), (B, 3000)], |_| {
        panic!("--no-wait waited")
    });
    gate.no_wait = true;

    let err = format!(
        "{:#}",
        slot::take(&config, &gate, None, &mut Vec::new()).unwrap_err()
    );

    assert!(err.starts_with("no free sandbox slot"), "{err}");
    assert!(!slot_file(&dir, 2).exists());
    assert_eq!(queue(&dir).len(), 1);
}

#[test]
fn a_slot_whose_holder_is_gone_is_taken_over_at_once_with_one_line() {
    let env = Env::new();
    let config = config(&env, Some(1));
    let dir = slots_dir(&env);
    write(
        &slot_file(&dir, 1),
        &holder(DEAD, 2000, "sbxm task run --issue 5", Some("issue-5")),
    );
    let (gate, sleeps) = gate(&env, vec![], |_| panic!("a gone holder was waited for"));
    let mut warn = Vec::new();
    let slot = slot::take(&config, &gate, None, &mut warn).unwrap();
    assert!(sleeps.lock().unwrap().is_empty());
    assert_eq!(
        String::from_utf8(warn).unwrap(),
        "took over sandbox slot 1 of sbxm task run --issue 5 (task issue-5, process 4100009, \
         since 2026-11-08T10:00:00Z), which is no longer running\n"
    );
    assert_eq!(
        read_holder(slot.path().unwrap()).process.pid,
        std::process::id()
    );
    assert!(!dir.join("slot-1.json.takeover").exists());
}

#[test]
fn a_slot_whose_pid_was_reused_by_another_process_is_taken_over_too() {
    let env = Env::new();
    let config = config(&env, Some(1));
    let dir = slots_dir(&env);
    // A is alive, but started at 2000, not at 1500 as the slot's holder did.
    write(
        &slot_file(&dir, 1),
        &holder(A, 1500, "sbxm task run --issue 7", Some("issue-7")),
    );
    let (gate, sleeps) = gate(&env, vec![(A, 2000)], |_| {
        panic!("a reused pid was waited for")
    });
    let mut warn = Vec::new();
    let slot = slot::take(&config, &gate, None, &mut warn).unwrap();
    assert!(sleeps.lock().unwrap().is_empty());
    let warn = String::from_utf8(warn).unwrap();
    assert_eq!(warn.lines().count(), 1, "{warn}");
    assert!(warn.starts_with("took over sandbox slot 1 of "), "{warn}");
    assert_eq!(
        read_holder(slot.path().unwrap()).process.pid,
        std::process::id()
    );
}

#[test]
fn a_ticket_whose_process_is_gone_does_not_hold_up_the_line() {
    let env = Env::new();
    let config = config(&env, Some(1));
    let dir = slots_dir(&env);
    let dead = dir
        .join("queue")
        .join(format!("{:032}-{DEAD:010}-000000.json", 1u64));
    write(&dead, &holder(DEAD, 2000, "sbxm run eval.toml", None));
    let (gate, sleeps) = gate(&env, vec![], |_| panic!("waited for a gone command"));
    let slot = slot::take(&config, &gate, None, &mut Vec::new()).unwrap();
    assert!(slot.path().is_some());
    assert!(sleeps.lock().unwrap().is_empty());
    assert!(!dead.exists());
}

#[test]
fn a_slot_file_left_unreadable_is_taken_over_at_the_next_look() {
    let env = Env::new();
    let config = config(&env, Some(1));
    let dir = slots_dir(&env);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(slot_file(&dir, 1), "").unwrap();
    let (gate, sleeps) = gate(&env, vec![], |_| {});
    let mut warn = Vec::new();
    let slot = slot::take(&config, &gate, None, &mut warn).unwrap();
    assert_eq!(sleeps.lock().unwrap().len(), 1);
    assert_eq!(
        read_holder(slot.path().unwrap()).process.pid,
        std::process::id()
    );
    let warn = String::from_utf8(warn).unwrap();
    assert!(
        warn.starts_with("took over sandbox slot 1 of an unreadable "),
        "{warn}"
    );
}

#[test]
fn ctrl_c_while_waiting_leaves_no_slot_file() {
    let env = Env::new();
    let config = config(&env, Some(1));
    let dir = slots_dir(&env);
    write(&slot_file(&dir, 1), &a_holder());
    let registry = Arc::new(Registry::default());
    let (released, queue_dir, slot_path) = (Arc::clone(&registry), dir.clone(), slot_file(&dir, 1));
    let (mut gate, _) = gate(&env, vec![(A, 2000)], move |n| {
        if n == 0 {
            assert_eq!(queue(&queue_dir).len(), 1, "the waiting command's ticket");
            // What the Ctrl-C handler does before the process exits.
            released.release();
            assert!(queue(&queue_dir).is_empty(), "{:?}", queue(&queue_dir));
            assert_eq!(read_holder(&slot_path).process.pid, A);
            // End the test's wait.
            std::fs::remove_file(&slot_path).unwrap();
        }
    });
    gate.registry = registry;
    slot::take(&config, &gate, None, &mut Vec::new()).unwrap();
}

#[test]
fn ctrl_c_while_holding_a_slot_gives_it_back() {
    let env = Env::new();
    let config = config(&env, Some(1));
    let (gate, _) = gate(&env, vec![], |_| {});
    let slot = slot::take(&config, &gate, None, &mut Vec::new()).unwrap();
    let path = slot.path().unwrap().to_owned();
    gate.registry.release();
    assert!(!path.exists());
}

#[test]
fn status_names_the_holder_in_one_line() {
    let env = Env::new();
    let (gate, _) = gate(&env, vec![(A, 2000)], |_| {});
    assert_eq!(slot::status_line(1, &gate), "sandbox slots: 0 of 1 held");
    let dir = slots_dir(&env);
    write(&slot_file(&dir, 1), &a_holder());
    assert_eq!(
        slot::status_line(1, &gate),
        format!("sandbox slots: 1 of 1 held by {A_LINE}")
    );
    // A gone waiter's ticket is neither counted nor removed: status only reads.
    let dead = dir
        .join("queue")
        .join(format!("{:032}-{DEAD:010}-000000.json", 1u64));
    write(&dead, &holder(DEAD, 2000, "sbxm run eval.toml", None));
    write(
        &dir.join("queue")
            .join(format!("{:032}-{B:010}-000000.json", 2u64)),
        &holder(B, 3000, "sbxm run eval.toml", None),
    );
    let (gate, _) = gate_with(&env, vec![(A, 2000), (B, 3000)]);
    assert_eq!(
        slot::status_line(1, &gate),
        format!("sandbox slots: 1 of 1 held by {A_LINE}, 1 waiting")
    );
    assert!(dead.exists());
}

fn gate_with(env: &Env, others: Vec<(u32, u64)>) -> (Gate, Sleeps) {
    gate(env, others, |_| {})
}
