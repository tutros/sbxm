//! `sbxm status` names who holds the machine's sandbox slots, in one line, when `sandbox_slots`
//! is set (issue 504, decision 304).

use crate::common;

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

fn env(slots: Option<(u32, sbxm::slot::Gate)>) -> Env {
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
        cloud_slots: None,
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
    assert!(Env::real(&unset).slots.is_none());
    common::slot::set_slots(&test, 2);
    let set = GlobalConfig::load(&test.config_dir()).unwrap();
    assert_eq!(Env::real(&set).slots.map(|(n, _)| n), Some(2));
}

#[test]
fn without_the_setting_there_is_no_slot_line() {
    let test = TestEnv::new();
    assert!(!frame(&test, &env(None)).contains("sandbox slot"));
}

#[test]
fn with_the_setting_one_line_names_the_holder() {
    let test = TestEnv::new();
    common::slot::full(&test, false);
    let text = frame(&test, &env(Some((1, common::slot::gate(&test, false)))));
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("sandbox slot"))
        .collect();
    assert_eq!(
        lines,
        vec![format!(
            "sandbox slots: 1 of 1 held by {}",
            common::slot::HOLDER
        )]
    );
}

#[test]
fn a_free_slot_says_so() {
    let test = TestEnv::new();
    let text = frame(&test, &env(Some((1, common::slot::gate(&test, false)))));
    assert!(text.contains("sandbox slots: 0 of 1 held\n"), "{text}");
}
