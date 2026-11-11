//! Issue 570: a slot wait that polls the loop's stop request (`Gate::stop`). A stop before the
//! wait, during it, or arriving just as the slot is taken gives up the ticket and the slot and
//! creates nothing ([`slot::Stopped`]); with no stop the slot is taken when it frees, as before.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::Env;
use sbxm::config::GlobalConfig;
use sbxm::slot::{self, Gate, Holder, Registry};
use sbxm::task::record::{Process, ProcessProbe};

/// This process started at 1000, the holder at 2000; every other pid is gone.
struct Probe;

const HOLDER: u32 = 4_100_001;

impl ProcessProbe for Probe {
    fn start_time(&self, pid: u32) -> Option<u64> {
        if pid == std::process::id() {
            Some(1000)
        } else if pid == HOLDER {
            Some(2000)
        } else {
            None
        }
    }
}

fn config(env: &Env) -> GlobalConfig {
    let path = env.config_dir().join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, format!("{text}sandbox_slots = 1\n")).unwrap();
    GlobalConfig::load(&env.config_dir()).unwrap()
}

fn slots_dir(env: &Env) -> PathBuf {
    env.tmp.path().join("local-data").join("sbxm").join("slots")
}

fn slot_file(env: &Env) -> PathBuf {
    slots_dir(env).join("slot-1.json")
}

fn queue(env: &Env) -> Vec<String> {
    match std::fs::read_dir(slots_dir(env).join("queue")) {
        Ok(entries) => entries
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn holder() -> Holder {
    Holder {
        process: Process::new(HOLDER, 2000),
        token: "t".to_owned(),
        command: "sbxm task review --pr 564 --force".to_owned(),
        task: Some("pr-564".to_owned()),
        since: "2026-11-10T10:00:00Z".to_owned(),
    }
}

/// The slot held by another live command.
fn hold(env: &Env) {
    std::fs::create_dir_all(slots_dir(env)).unwrap();
    std::fs::write(slot_file(env), serde_json::to_string(&holder()).unwrap()).unwrap();
}

fn read_holder(path: &Path) -> Holder {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The loop's stop request, as a flag the test sets.
#[derive(Clone, Default)]
struct Stop(Arc<AtomicBool>);

impl Stop {
    fn ask(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

type Sleeps = Arc<Mutex<usize>>;

/// A gate polling `stop`; `on_sleep` runs at each wait, given how many came before.
fn gate(
    stop: &Stop,
    env: &Env,
    on_sleep: impl Fn(usize) + Send + Sync + 'static,
) -> (Gate, Sleeps) {
    let sleeps: Sleeps = Arc::default();
    let seen = Arc::clone(&sleeps);
    let asked = stop.clone();
    let gate = Gate {
        dir: Some(slots_dir(env)),
        no_wait: false,
        probe: Arc::new(Probe),
        sleep: Arc::new(move |_| {
            let n = {
                let mut seen = seen.lock().unwrap();
                *seen += 1;
                *seen - 1
            };
            assert!(n < 20, "waited too long");
            on_sleep(n);
        }),
        command: "sbxm task loop".to_owned(),
        catch_ctrl_c: false,
        registry: Arc::new(Registry::default()),
        stop: Some(Arc::new(move || asked.0.load(Ordering::SeqCst))),
    };
    (gate, sleeps)
}

fn stopped(result: anyhow::Result<slot::Slot>) -> String {
    let e = result.expect_err("stopped");
    assert!(e.is::<slot::Stopped>(), "{e:#}");
    format!("{e:#}")
}

#[test]
fn a_stop_before_the_wait_writes_no_ticket_and_never_waits() {
    let env = Env::new();
    let config = config(&env);
    hold(&env);
    let stop = Stop::default();
    stop.ask();
    let (gate, sleeps) = gate(&stop, &env, |_| panic!("waited"));
    let mut warn = Vec::new();

    let e = stopped(slot::take(&config, &gate, Some("issue-550"), &mut warn));

    assert!(e.contains("nothing was created"), "{e}");
    assert!(queue(&env).is_empty(), "{:?}", queue(&env));
    assert_eq!(
        read_holder(&slot_file(&env)),
        holder(),
        "the holder keeps it"
    );
    assert_eq!(*sleeps.lock().unwrap(), 0);
    assert!(warn.is_empty(), "{}", String::from_utf8_lossy(&warn));
}

#[test]
fn a_stop_before_the_take_of_a_free_slot_takes_nothing() {
    let env = Env::new();
    let config = config(&env);
    let stop = Stop::default();
    stop.ask();
    let (gate, _) = gate(&stop, &env, |_| panic!("waited"));

    stopped(slot::take(
        &config,
        &gate,
        Some("issue-550"),
        &mut Vec::new(),
    ));

    assert!(!slot_file(&env).exists());
    assert!(queue(&env).is_empty());
}

#[test]
fn a_stop_during_the_wait_gives_up_the_ticket() {
    let env = Env::new();
    let config = config(&env);
    hold(&env);
    let stop = Stop::default();
    let asked = stop.clone();
    let tickets = Arc::new(Mutex::new(Vec::new()));
    let (seen, queued) = (Arc::clone(&tickets), slots_dir(&env).join("queue"));
    let (gate, sleeps) = gate(&stop, &env, move |n| {
        seen.lock()
            .unwrap()
            .push(std::fs::read_dir(&queued).unwrap().count());
        if n == 1 {
            asked.ask();
        }
    });
    let mut warn = Vec::new();

    stopped(slot::take(&config, &gate, Some("issue-550"), &mut warn));

    assert_eq!(*tickets.lock().unwrap(), [1, 1], "it waited in line");
    assert_eq!(*sleeps.lock().unwrap(), 2, "no wait after the stop");
    assert!(queue(&env).is_empty(), "{:?}", queue(&env));
    assert_eq!(
        read_holder(&slot_file(&env)),
        holder(),
        "the holder keeps it"
    );
    assert!(
        String::from_utf8(warn)
            .unwrap()
            .starts_with("waiting for a sandbox slot: held by sbxm task review --pr 564")
    );
}

#[test]
fn a_stop_arriving_as_the_slot_frees_releases_it_once_taken() {
    let env = Env::new();
    let config = config(&env);
    hold(&env);
    let stop = Stop::default();
    let asked = stop.clone();
    let path = slot_file(&env);
    // The holder ends and the stop comes in the same pause: the next look takes the slot.
    let (gate, sleeps) = gate(&stop, &env, move |_| {
        std::fs::remove_file(&path).unwrap();
        asked.ask();
    });

    stopped(slot::take(
        &config,
        &gate,
        Some("issue-550"),
        &mut Vec::new(),
    ));

    assert_eq!(*sleeps.lock().unwrap(), 1);
    assert!(!slot_file(&env).exists(), "the slot is free again");
    assert!(queue(&env).is_empty());
}

#[test]
fn with_no_stop_the_slot_is_taken_when_it_frees() {
    let env = Env::new();
    let config = config(&env);
    hold(&env);
    let stop = Stop::default();
    let path = slot_file(&env);
    let (gate, sleeps) = gate(&stop, &env, move |n| {
        if n == 1 {
            std::fs::remove_file(&path).unwrap();
        }
    });

    let slot = slot::take(&config, &gate, Some("issue-550"), &mut Vec::new()).unwrap();

    assert_eq!(*sleeps.lock().unwrap(), 2);
    let held = read_holder(slot.path().unwrap());
    assert_eq!(held.process.pid, std::process::id());
    assert_eq!(held.task.as_deref(), Some("issue-550"));
    assert!(queue(&env).is_empty());
}

#[test]
fn a_stop_while_this_command_holds_the_slot_keeps_it_for_its_next_step() {
    let env = Env::new();
    let config = config(&env);
    let stop = Stop::default();
    let (gate, _) = gate(&stop, &env, |_| panic!("waited on itself"));
    let outer = slot::take(&config, &gate, Some("issue-550"), &mut Vec::new()).unwrap();
    stop.ask();

    // `task run` going on to its review: the task already started, so it goes on.
    let inner = slot::take(&config, &gate, Some("issue-550"), &mut Vec::new()).unwrap();

    assert_eq!(inner.path(), outer.path());
}

#[test]
fn a_gate_without_a_stop_request_waits_as_before() {
    let env = Env::new();
    let config = config(&env);
    hold(&env);
    let path = slot_file(&env);
    let (mut gate, sleeps) = gate(&Stop::default(), &env, move |_| {
        std::fs::remove_file(&path).unwrap();
    });
    gate.stop = None;

    slot::take(&config, &gate, None, &mut Vec::new()).unwrap();

    assert_eq!(*sleeps.lock().unwrap(), 1);
    assert_eq!(Duration::from_secs(5), slot::POLL);
}
