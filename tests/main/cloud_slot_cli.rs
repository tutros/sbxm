//! `--no-wait` on every command that takes a cloud slot names `cloud_slots` too (issue 599).

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
fn every_cloud_slot_taking_command_names_cloud_slots_at_no_wait() {
    for args in [
        &["task", "run"][..],
        &["task", "start"],
        &["task", "review"],
        &["task", "fix"],
        &["task", "resume"],
        &["task", "gates"],
    ] {
        let text = help(args);
        assert!(text.contains("--no-wait"), "{args:?}: {text}");
        assert!(text.contains("cloud_slots"), "{args:?}: {text}");
    }
}

#[test]
fn local_only_commands_name_only_sandbox_slots() {
    for args in [&["run"][..], &["template", "build"]] {
        assert!(!help(args).contains("cloud_slots"), "{args:?}");
    }
}
