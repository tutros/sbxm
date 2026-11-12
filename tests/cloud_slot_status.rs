//! `sbxm status` names who holds the machine's cloud slots, in one line next to the sandbox
//! slots' line, when `cloud_slots` is set (issue 599).

mod common;

use common::Env as TestEnv;
use sbxm::backend::FakeBackend;
use sbxm::commands::status::{self, Env};
use sbxm::config::GlobalConfig;
use sbxm::status::footer::Disk;
use sbxm::task::record::ProcessProbe;

struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

type Slots = Option<(u32, sbxm::slot::Gate)>;

fn env(slots: Slots, cloud_slots: Slots) -> Env {
    Env {
        now: Box::new(|| 1_790_000_000),
        sleep: Box::new(|_| {}),
        terminal: false,
        width: 80,
        iterations: None,
        monotonic: Box::new(|| std::time::Duration::ZERO),
        disk: Disk {
            free_space: |_| Ok(u64::MAX),
            same_drive: |_, _| true,
            state_dir: None,
            min_free_disk: 0,
        },
        slots,
        cloud_slots,
    }
}

fn frame(test: &TestEnv, env: &Env) -> String {
    let mut out = Vec::new();
    status::run(
        &test.base_dir(),
        &FakeBackend::default(),
        &Gone,
        env,
        None,
        &mut out,
    )
    .unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn the_real_env_reads_the_setting() {
    let test = TestEnv::new();
    let unset = GlobalConfig::load(&test.config_dir()).unwrap();
    assert!(Env::real(&unset).cloud_slots.is_none());
    common::cloud_slot::set_cloud_slots(&test, 3);
    let set = GlobalConfig::load(&test.config_dir()).unwrap();
    let real = Env::real(&set);
    assert_eq!(real.cloud_slots.map(|(n, _)| n), Some(3));
    assert!(real.slots.is_none());
}

#[test]
fn without_the_setting_there_is_no_cloud_line() {
    let test = TestEnv::new();
    let gate = common::slot::gate(&test, false);
    assert!(!frame(&test, &env(Some((1, gate)), None)).contains("cloud slot"));
}

#[test]
fn with_the_setting_one_line_names_the_holder_after_the_sandbox_line() {
    let test = TestEnv::new();
    common::cloud_slot::full(&test, false);
    let gate = common::slot::gate(&test, false);
    let text = frame(&test, &env(Some((1, gate.clone())), Some((1, gate))));
    let lines: Vec<&str> = text.lines().filter(|l| l.contains(" slots: ")).collect();
    assert_eq!(
        lines,
        vec![
            "sandbox slots: 0 of 1 held".to_owned(),
            format!("cloud slots: 1 of 1 held by {}", common::cloud_slot::HOLDER),
        ]
    );
}

#[test]
fn the_cloud_line_shows_without_the_sandbox_line() {
    let test = TestEnv::new();
    let text = frame(
        &test,
        &env(None, Some((2, common::slot::gate(&test, false)))),
    );
    assert!(text.contains("cloud slots: 0 of 2 held\n"), "{text}");
    assert!(!text.contains("sandbox slots"), "{text}");
}
