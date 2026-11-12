//! The machine's cloud slots for command tests (issue 599): `cloud_slots = 1` in the test config,
//! and a cloud slots folder of the test's own, free or held by another live command. The gate is
//! [`super::slot::gate`]'s, whose cloud folder sits next to its sandbox slots folder.

use std::path::PathBuf;
use std::sync::Arc;

use sbxm::slot::Holder;
use sbxm::task::record::Process;

use super::Env;
use super::slot::{HOLDER_PID, gate};

/// How a refusal or a wait names [`full`]'s holder.
pub const HOLDER: &str = "sbxm task run --issue 7 --cloud (task issue-7, process 4100001, since \
                          2026-11-12T10:00:00Z)";

/// Sets `cloud_slots = n` in `[resources]` of `env`'s config.
pub fn set_cloud_slots(env: &Env, n: u32) {
    let path = env.config_dir().join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[resources]\n"), "{text}");
    let text = text.replacen(
        "[resources]\n",
        &format!("[resources]\ncloud_slots = {n}\n"),
        1,
    );
    std::fs::write(&path, text).unwrap();
}

pub fn cloud_slots_dir(env: &Env) -> PathBuf {
    env.tmp
        .path()
        .join("local-data")
        .join("sbxm")
        .join("cloud-slots")
}

/// The one cloud slot's file.
pub fn cloud_slot_file(env: &Env) -> PathBuf {
    cloud_slots_dir(env).join("slot-1.json")
}

/// Who holds the cloud slot now, if anyone.
pub fn holder(env: &Env) -> Option<Holder> {
    let text = std::fs::read_to_string(cloud_slot_file(env)).ok()?;
    Some(serde_json::from_str(&text).unwrap())
}

/// The tickets of commands waiting in line for a cloud slot.
pub fn queue(env: &Env) -> Vec<String> {
    match std::fs::read_dir(cloud_slots_dir(env).join("queue")) {
        Ok(entries) => entries
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// A disk guard whose gate never waits, one cloud slot set and free.
pub fn free(env: &Env) -> sbxm::disk::Guard {
    set_cloud_slots(env, 1);
    sbxm::disk::Guard {
        slot: gate(env, false),
        ..Default::default()
    }
}

/// A disk guard whose gate never waits, one cloud slot set and held by another live command
/// ([`HOLDER`]).
pub fn full(env: &Env, no_wait: bool) -> sbxm::disk::Guard {
    set_cloud_slots(env, 1);
    let holder = Holder {
        process: Process::new(HOLDER_PID, 2000),
        token: "t".to_owned(),
        command: "sbxm task run --issue 7 --cloud".to_owned(),
        task: Some("issue-7".to_owned()),
        since: "2026-11-12T10:00:00Z".to_owned(),
    };
    std::fs::create_dir_all(cloud_slots_dir(env)).unwrap();
    std::fs::write(
        cloud_slot_file(env),
        serde_json::to_string(&holder).unwrap(),
    )
    .unwrap();
    sbxm::disk::Guard {
        slot: gate(env, no_wait),
        ..Default::default()
    }
}

/// [`full`], whose holder ends at the first wait.
pub fn full_until_first_wait(env: &Env) -> sbxm::disk::Guard {
    let mut guard = full(env, false);
    let path = cloud_slot_file(env);
    guard.slot.sleep = Arc::new(move |_| std::fs::remove_file(&path).unwrap());
    guard
}

/// The start of a `--no-wait` refusal of [`full`].
pub fn refusal() -> String {
    format!("no free cloud slot (cloud_slots = 1 in config.toml): held by {HOLDER}")
}
