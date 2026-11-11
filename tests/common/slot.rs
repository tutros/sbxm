//! The machine's sandbox slots for command tests (issue 504, decision 304): `sandbox_slots = 1`
//! in the test config, and a slots folder of the test's own, free or held by another live command.

use std::path::PathBuf;
use std::sync::Arc;

use sbxm::slot::{Gate, Holder, Registry};
use sbxm::task::record::{Process, ProcessProbe};

use super::Env;

/// The live command holding the slot in [`full`].
pub const HOLDER_PID: u32 = 4_100_001;

/// How a refusal or a wait names [`full`]'s holder.
pub const HOLDER: &str = "sbxm task run --issue 7 (task issue-7, process 4100001, since \
                          2026-11-08T10:00:00Z)";

/// This process started at 1000, the holder at 2000; every other pid is gone.
struct Probe;

impl ProcessProbe for Probe {
    fn start_time(&self, pid: u32) -> Option<u64> {
        if pid == std::process::id() {
            Some(1000)
        } else if pid == HOLDER_PID {
            Some(2000)
        } else {
            None
        }
    }
}

/// Sets `sandbox_slots = n` in `[resources]` of `env`'s config.
pub fn set_slots(env: &Env, n: u32) {
    let path = env.config_dir().join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[resources]\n"), "{text}");
    let text = text.replacen(
        "[resources]\n",
        &format!("[resources]\nsandbox_slots = {n}\n"),
        1,
    );
    std::fs::write(&path, text).unwrap();
}

pub fn slots_dir(env: &Env) -> PathBuf {
    env.tmp.path().join("local-data").join("sbxm").join("slots")
}

/// The one slot's file.
pub fn slot_file(env: &Env) -> PathBuf {
    slots_dir(env).join("slot-1.json")
}

/// Who holds the slot now, if anyone.
pub fn holder(env: &Env) -> Option<Holder> {
    let text = std::fs::read_to_string(slot_file(env)).ok()?;
    Some(serde_json::from_str(&text).unwrap())
}

/// The tickets of commands waiting in line.
pub fn queue(env: &Env) -> Vec<String> {
    match std::fs::read_dir(slots_dir(env).join("queue")) {
        Ok(entries) => entries
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// A gate over `env`'s slots folder that must never wait.
pub fn gate(env: &Env, no_wait: bool) -> Gate {
    Gate {
        dir: Some(slots_dir(env)),
        no_wait,
        probe: Arc::new(Probe),
        sleep: Arc::new(|_| panic!("waited for a sandbox slot")),
        command: "sbxm under test".to_owned(),
        catch_ctrl_c: false,
        registry: Arc::new(Registry::default()),
        stop: None,
    }
}

/// A disk guard with [`gate`], one slot set and free.
pub fn free(env: &Env) -> sbxm::disk::Guard {
    set_slots(env, 1);
    sbxm::disk::Guard {
        slot: gate(env, false),
        ..Default::default()
    }
}

/// A disk guard with [`gate`], one slot set and held by another live command ([`HOLDER`]).
pub fn full(env: &Env, no_wait: bool) -> sbxm::disk::Guard {
    set_slots(env, 1);
    let holder = Holder {
        process: Process::new(HOLDER_PID, 2000),
        token: "t".to_owned(),
        command: "sbxm task run --issue 7".to_owned(),
        task: Some("issue-7".to_owned()),
        since: "2026-11-08T10:00:00Z".to_owned(),
    };
    std::fs::create_dir_all(slots_dir(env)).unwrap();
    std::fs::write(slot_file(env), serde_json::to_string(&holder).unwrap()).unwrap();
    sbxm::disk::Guard {
        slot: gate(env, no_wait),
        ..Default::default()
    }
}

/// [`full`], whose holder ends at the first wait.
pub fn full_until_first_wait(env: &Env) -> sbxm::disk::Guard {
    let mut guard = full(env, false);
    let path = slot_file(env);
    guard.slot.sleep = Arc::new(move |_| std::fs::remove_file(&path).unwrap());
    guard
}

/// The start of a `--no-wait` refusal of [`full`].
pub fn refusal() -> String {
    format!("no free sandbox slot (sandbox_slots = 1 in config.toml): held by {HOLDER}")
}
