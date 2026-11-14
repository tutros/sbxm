//! `--no-wait` on every command that takes the machine's sandbox slot, and on none other
//! (issue 504, decision 304).

use assert_cmd::Command;

fn help(args: &[&str]) -> String {
    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .args(args)
        .arg("--help")
        .assert()
        .success()
        .get_output()
        .clone();
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn every_slot_taking_command_has_no_wait() {
    for args in [
        &["task", "run"][..],
        &["task", "start"],
        &["task", "review"],
        &["task", "fix"],
        &["task", "resume"],
        &["task", "gates"],
        &["run"],
        &["template", "build"],
    ] {
        let text = help(args);
        assert!(text.contains("--no-wait"), "{args:?}: {text}");
        assert!(text.contains("sandbox_slots"), "{args:?}: {text}");
    }
}

#[test]
fn new_and_open_have_no_no_wait() {
    for args in [&["new"][..], &["open"]] {
        assert!(!help(args).contains("--no-wait"), "{args:?}");
    }
}
