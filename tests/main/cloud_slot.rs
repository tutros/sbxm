//! The machine's cloud slots (issue 599, decisions 304 and 317): `[resources] cloud_slots`
//! limits how many cloud commands run at once, in a pool of its own next to the sandbox slots.

use crate::common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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

const A: u32 = 4_200_001;
const B: u32 = 4_200_002;
const DEAD: u32 = 4_200_009;

/// The test config with `lines` added to its `[resources]` table.
fn config(env: &Env, lines: &[&str]) -> GlobalConfig {
    let path = env.config_dir().join("config.toml");
    let mut text = std::fs::read_to_string(&path).unwrap();
    for line in lines {
        text.push_str(&format!("{line}\n"));
    }
    std::fs::write(&path, text).unwrap();
    GlobalConfig::load(&env.config_dir()).unwrap()
}

fn local_data(env: &Env) -> PathBuf {
    env.tmp.path().join("local-data").join("sbxm")
}

fn cloud_dir(env: &Env) -> PathBuf {
    local_data(env).join("cloud-slots")
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
        dir: Some(local_data(env).join("slots")),
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
        command: "sbxm task run --issue 9 --cloud".to_owned(),
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
        since: "2026-11-12T10:00:00Z".to_owned(),
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
    holder(A, 2000, "sbxm task run --issue 7 --cloud", Some("issue-7"))
}

const A_LINE: &str = "sbxm task run --issue 7 --cloud (task issue-7, process 4200001, since \
                      2026-11-12T10:00:00Z)";

#[test]
fn unset_lets_two_cloud_commands_run_at_once_writing_no_file() {
    let env = Env::new();
    let config = config(&env, &[]);
    let (gate, sleeps) = gate(&env, vec![], |_| panic!("no wait without a limit"));
    let first = slot::take_cloud(&config, &gate, Some("issue-1"), &mut Vec::new()).unwrap();
    let second = std::thread::scope(|s| {
        s.spawn(|| slot::take_cloud(&config, &gate, Some("issue-2"), &mut Vec::new()).unwrap())
            .join()
            .unwrap()
    });
    assert_eq!(first.path(), None);
    assert_eq!(second.path(), None);
    assert!(!env.tmp.path().join("local-data").exists());
    assert!(sleeps.lock().unwrap().is_empty());
}

#[test]
fn sandbox_slots_alone_leaves_cloud_commands_unlimited() {
    let env = Env::new();
    let config = config(&env, &["sandbox_slots = 1"]);
    let (gate, _) = gate(&env, vec![], |_| panic!("no wait without a cloud limit"));
    let cloud = slot::take_cloud(&config, &gate, None, &mut Vec::new()).unwrap();
    assert_eq!(cloud.path(), None);
    assert!(!env.tmp.path().join("local-data").exists());
}

#[test]
fn a_free_cloud_slot_is_taken_in_its_own_folder_and_given_back_on_drop() {
    let env = Env::new();
    let config = config(&env, &["cloud_slots = 1"]);
    let (gate, sleeps) = gate(&env, vec![], |_| panic!("a free slot doesn't wait"));
    let mut warn = Vec::new();
    let slot = slot::take_cloud(&config, &gate, Some("issue-3"), &mut warn).unwrap();
    let path = slot_file(&cloud_dir(&env), 1);
    assert_eq!(slot.path(), Some(path.as_path()));
    let held = read_holder(&path);
    assert_eq!(held.process.pid, std::process::id());
    assert_eq!(held.task.as_deref(), Some("issue-3"));
    assert!(!local_data(&env).join("slots").exists(), "no local slot");
    assert!(warn.is_empty());
    assert!(sleeps.lock().unwrap().is_empty());
    drop(slot);
    assert!(!path.exists());
}

#[test]
fn the_real_cloud_folder_is_next_to_the_sandbox_slots_folder() {
    let local = slot::slots_dir().unwrap();
    let cloud = slot::cloud_slots_dir().unwrap();
    assert_eq!(cloud.parent(), local.parent());
    assert_eq!(cloud.file_name().unwrap(), "cloud-slots");
}

#[test]
fn a_cloud_command_already_holding_its_slot_never_waits_on_itself() {
    let env = Env::new();
    let config = config(&env, &["cloud_slots = 1"]);
    let (gate, _) = gate(&env, vec![], |_| panic!("waited on itself"));
    // A cloud `task run` going on to its review: the reviewer takes no second slot.
    let outer = slot::take_cloud(&config, &gate, Some("issue-4"), &mut Vec::new()).unwrap();
    let inner = slot::take_cloud(&config, &gate, Some("issue-4"), &mut Vec::new()).unwrap();
    assert_eq!(inner.path(), outer.path());
    drop(inner);
    let path = slot_file(&cloud_dir(&env), 1);
    assert!(path.exists(), "the outer command still holds it");
    drop(outer);
    assert!(!path.exists());
}

#[test]
fn a_local_and_a_cloud_command_run_at_once_in_separate_pools() {
    let env = Env::new();
    let config = config(&env, &["sandbox_slots = 1", "cloud_slots = 1"]);
    let (gate, _) = gate(&env, vec![], |_| panic!("the pools mixed"));
    let local = slot::take(&config, &gate, Some("issue-5"), &mut Vec::new()).unwrap();
    // Another command, on another thread, at the same time.
    let cloud_path = std::thread::scope(|s| {
        s.spawn(|| {
            let cloud = slot::take_cloud(&config, &gate, Some("issue-6"), &mut Vec::new()).unwrap();
            cloud.path().unwrap().to_owned()
        })
        .join()
        .unwrap()
    });
    assert_eq!(
        local.path(),
        Some(slot_file(&local_data(&env).join("slots"), 1).as_path())
    );
    assert_eq!(cloud_path, slot_file(&cloud_dir(&env), 1));
}

#[test]
fn a_held_local_slot_is_not_a_cloud_slot_in_the_same_thread() {
    let env = Env::new();
    let config = config(&env, &["sandbox_slots = 1", "cloud_slots = 1"]);
    let (gate, _) = gate(&env, vec![], |_| panic!("waited"));
    let local = slot::take(&config, &gate, None, &mut Vec::new()).unwrap();
    let cloud = slot::take_cloud(&config, &gate, None, &mut Vec::new()).unwrap();
    assert_ne!(local.path(), cloud.path());
    assert_eq!(cloud.path(), Some(slot_file(&cloud_dir(&env), 1).as_path()));
}

#[test]
fn a_second_cloud_command_waits_naming_the_holder_then_runs() {
    let env = Env::new();
    let config = config(&env, &["cloud_slots = 1"]);
    let dir = cloud_dir(&env);
    write(&slot_file(&dir, 1), &a_holder());
    let released = slot_file(&dir, 1);
    let (gate, sleeps) = gate(&env, vec![(A, 2000)], move |n| {
        if n == 1 {
            std::fs::remove_file(&released).unwrap();
        }
    });
    let mut warn = Vec::new();
    let slot = slot::take_cloud(&config, &gate, None, &mut warn).unwrap();
    assert_eq!(
        String::from_utf8(warn).unwrap(),
        format!("waiting for a cloud slot: held by {A_LINE}\n")
    );
    assert_eq!(*sleeps.lock().unwrap(), vec![slot::POLL; 2]);
    assert_eq!(
        read_holder(slot.path().unwrap()).process.pid,
        std::process::id()
    );
    assert!(queue(&dir).is_empty(), "its ticket is gone");
}

#[test]
fn no_wait_with_no_free_cloud_slot_refuses_naming_the_holder_and_writes_nothing() {
    let env = Env::new();
    let config = config(&env, &["cloud_slots = 1"]);
    let dir = cloud_dir(&env);
    write(&slot_file(&dir, 1), &a_holder());
    let (mut gate, _) = gate(&env, vec![(A, 2000)], |_| panic!("--no-wait waited"));
    gate.no_wait = true;
    let err = slot::take_cloud(&config, &gate, None, &mut Vec::new()).unwrap_err();
    let err = format!("{err:#}");
    assert!(
        err.starts_with(&format!(
            "no free cloud slot (cloud_slots = 1 in config.toml): held by {A_LINE}"
        )),
        "{err}"
    );
    assert!(err.contains("--no-wait"), "{err}");
    assert_eq!(read_holder(&slot_file(&dir, 1)), a_holder());
    assert!(queue(&dir).is_empty());
}

#[test]
fn two_cloud_slots_let_two_run_and_a_third_waits_behind_an_older_waiter() {
    let env = Env::new();
    let config = config(&env, &["cloud_slots = 2"]);
    let dir = cloud_dir(&env);
    write(&slot_file(&dir, 1), &a_holder());
    let (gate2, _) = gate(&env, vec![(A, 2000)], |_| panic!("slot 2 was free"));
    let second = slot::take_cloud(&config, &gate2, None, &mut Vec::new()).unwrap();
    assert_eq!(second.path(), Some(slot_file(&dir, 2).as_path()));

    // B came first and waits; the third command goes after it, first come first served.
    let b_ticket = dir
        .join("queue")
        .join(format!("{:032}-{B:010}-000000.json", 1u64));
    let b = holder(B, 3000, "sbxm task review --pr 5", Some("pr-5"));
    write(&b_ticket, &b);
    let third = std::thread::scope(|s| {
        s.spawn(|| {
            let (slot_path, ticket) = (slot_file(&dir, 1), b_ticket.clone());
            let (gate3, sleeps) = gate(&env, vec![(A, 2000), (B, 3000)], move |n| match n {
                // A ends: the slot is free, but B is in line first, and takes it.
                0 => std::fs::remove_file(&slot_path).unwrap(),
                1 => {
                    assert!(!slot_path.exists(), "the later command took B's turn");
                    write(&slot_path, &b);
                    std::fs::remove_file(&ticket).unwrap();
                }
                // B ends.
                2 => std::fs::remove_file(&slot_path).unwrap(),
                _ => {}
            });
            let slot = slot::take_cloud(&config, &gate3, None, &mut Vec::new()).unwrap();
            assert_eq!(sleeps.lock().unwrap().len(), 3);
            slot.path().unwrap().to_owned()
        })
        .join()
        .unwrap()
    });
    assert_eq!(third, slot_file(&dir, 1));
}

#[test]
fn a_cloud_slot_whose_holder_is_gone_is_taken_over_with_one_line() {
    let env = Env::new();
    let config = config(&env, &["cloud_slots = 1"]);
    let dir = cloud_dir(&env);
    write(
        &slot_file(&dir, 1),
        &holder(
            DEAD,
            2000,
            "sbxm task run --issue 5 --cloud",
            Some("issue-5"),
        ),
    );
    let (gate, sleeps) = gate(&env, vec![], |_| panic!("a gone holder was waited for"));
    let mut warn = Vec::new();
    let slot = slot::take_cloud(&config, &gate, None, &mut warn).unwrap();
    assert!(sleeps.lock().unwrap().is_empty());
    assert_eq!(
        String::from_utf8(warn).unwrap(),
        "took over cloud slot 1 of sbxm task run --issue 5 --cloud (task issue-5, process \
         4200009, since 2026-11-12T10:00:00Z), which is no longer running\n"
    );
    assert_eq!(
        read_holder(slot.path().unwrap()).process.pid,
        std::process::id()
    );
}

#[test]
fn a_stop_while_waiting_for_a_cloud_slot_gives_up_with_nothing_created() {
    let env = Env::new();
    let config = config(&env, &["cloud_slots = 1"]);
    let dir = cloud_dir(&env);
    write(&slot_file(&dir, 1), &a_holder());
    let asked = Arc::new(AtomicBool::new(false));
    let ask = Arc::clone(&asked);
    let (mut gate, sleeps) = gate(&env, vec![(A, 2000)], move |_| {
        ask.store(true, Ordering::SeqCst);
    });
    gate.stop = Some(Arc::new(move || asked.load(Ordering::SeqCst)));
    let e = slot::take_cloud(&config, &gate, Some("issue-8"), &mut Vec::new()).unwrap_err();
    assert!(e.is::<slot::Stopped>(), "{e:#}");
    let e = format!("{e:#}");
    assert!(e.contains("nothing was created"), "{e}");
    assert_eq!(sleeps.lock().unwrap().len(), 1);
    assert!(queue(&dir).is_empty(), "{:?}", queue(&dir));
    assert_eq!(read_holder(&slot_file(&dir, 1)), a_holder());
}

#[test]
fn status_names_the_cloud_holders_in_one_line() {
    let env = Env::new();
    let (gate, _) = gate(&env, vec![(A, 2000), (B, 3000)], |_| {});
    assert_eq!(
        slot::cloud_status_line(2, &gate),
        "cloud slots: 0 of 2 held"
    );
    let dir = cloud_dir(&env);
    write(&slot_file(&dir, 1), &a_holder());
    write(
        &dir.join("queue")
            .join(format!("{:032}-{B:010}-000000.json", 2u64)),
        &holder(B, 3000, "sbxm task review --pr 5", None),
    );
    assert_eq!(
        slot::cloud_status_line(2, &gate),
        format!("cloud slots: 1 of 2 held by {A_LINE}, 1 waiting")
    );
    // The local line reads its own folder only.
    assert_eq!(slot::status_line(1, &gate), "sandbox slots: 0 of 1 held");
}

// ---- Review of issue 599, M-1: `slot::hold` keeps a step's slot to the end of the command ----

#[test]
fn a_hold_keeps_a_step_s_slot_until_it_ends_and_the_next_step_reuses_it() {
    let env = Env::new();
    let config = config(&env, &["cloud_slots = 1"]);
    let (gate, _) = gate(&env, vec![], |_| {
        panic!("the next step waited on its own command")
    });
    let path = slot_file(&cloud_dir(&env), 1);
    let hold = slot::hold();
    // The start takes it after its own checks, and ends.
    let start = slot::take_cloud(&config, &gate, Some("issue-1"), &mut Vec::new()).unwrap();
    drop(start);
    assert!(path.exists(), "kept between the steps");
    // The review finds it held.
    let review = slot::take_cloud(&config, &gate, Some("issue-1"), &mut Vec::new()).unwrap();
    assert_eq!(review.path(), Some(path.as_path()));
    drop(review);
    assert!(path.exists());
    drop(hold);
    assert!(!path.exists(), "given back with the hold");
}

#[test]
fn without_a_hold_a_step_s_slot_goes_when_it_ends() {
    let env = Env::new();
    let config = config(&env, &["cloud_slots = 1"]);
    let (gate, _) = gate(&env, vec![], |_| panic!("waited"));
    let path = slot_file(&cloud_dir(&env), 1);
    {
        let _hold = slot::hold();
    }
    let step = slot::take_cloud(&config, &gate, None, &mut Vec::new()).unwrap();
    drop(step);
    assert!(!path.exists());
}

#[test]
fn an_inner_hold_gives_back_nothing_the_outer_one_still_keeps() {
    let env = Env::new();
    let config = config(&env, &["cloud_slots = 1"]);
    let (gate, _) = gate(&env, vec![], |_| panic!("waited"));
    let path = slot_file(&cloud_dir(&env), 1);
    let outer = slot::hold();
    let inner = slot::hold();
    drop(slot::take_cloud(&config, &gate, None, &mut Vec::new()).unwrap());
    drop(inner);
    assert!(path.exists(), "the outer hold keeps it");
    drop(outer);
    assert!(!path.exists());
}
