use std::path::PathBuf;

use sbxm::backend::{ExecOutput, ExecSpec, FakeBackend, SandboxBackend, Stdin};
use serde_json::json;

fn spec(workdir: Option<&str>, stdin: Stdin) -> ExecSpec {
    ExecSpec {
        workdir: workdir.map(PathBuf::from),
        argv: vec!["echo".into(), "hi".into()],
        stdin,
    }
}

#[test]
fn fake_records_exec_calls_with_workdir_and_stdin_mode() {
    let fake = FakeBackend::default();
    fake.exec("sb-1", &spec(Some("/work"), Stdin::Closed))
        .unwrap();
    fake.exec("sb-2", &spec(None, Stdin::Piped(String::new())))
        .unwrap();

    let calls = fake.execs();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "sb-1");
    assert_eq!(calls[0].1.workdir, Some(PathBuf::from("/work")));
    assert_eq!(calls[0].1.stdin, Stdin::Closed);
    assert_eq!(calls[1].1.stdin, Stdin::Piped(String::new()));
    assert_eq!(fake.log(), ["exec sb-1", "exec sb-2"]);
}

#[test]
fn fake_returns_scripted_exec_outputs_in_order() {
    let scripted = |code, out: &str| ExecOutput {
        stdout: out.into(),
        stderr: "err".into(),
        exit_code: Some(code),
    };
    let fake = FakeBackend::default().with_exec_outputs(vec![scripted(0, "a"), scripted(124, "b")]);

    let first = fake.exec("s", &spec(None, Stdin::Closed)).unwrap();
    let second = fake.exec("s", &spec(None, Stdin::Closed)).unwrap();
    assert_eq!(first, scripted(0, "a"));
    assert_eq!(second, scripted(124, "b"));
}

#[test]
fn fake_exec_defaults_to_empty_success() {
    let out = FakeBackend::default()
        .exec("s", &spec(None, Stdin::Closed))
        .unwrap();
    assert_eq!(out.exit_code, Some(0));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
}

#[test]
fn fake_skills_returns_scripted_json_and_records_the_call() {
    let fake = FakeBackend::default().with_skills(json!({"skills": []}));
    assert_eq!(fake.skills().unwrap(), json!({"skills": []}));
    assert_eq!(fake.log(), ["skills"]);
}

#[test]
fn fake_can_be_shared_across_threads() {
    let fake = FakeBackend::default();
    std::thread::scope(|s| {
        s.spawn(|| fake.exec("a", &spec(None, Stdin::Closed)).unwrap());
        s.spawn(|| fake.exec("b", &spec(None, Stdin::Closed)).unwrap());
    });
    let mut names: Vec<_> = fake.execs().into_iter().map(|(n, _)| n).collect();
    names.sort();
    assert_eq!(names, ["a", "b"]);
}
