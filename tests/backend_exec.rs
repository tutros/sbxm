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

// ---- per-sandbox scripting and the exec gate (slice 6) --------------------

fn out(stdout: &str) -> ExecOutput {
    ExecOutput {
        stdout: stdout.into(),
        stderr: String::new(),
        exit_code: Some(0),
    }
}

#[test]
fn fake_scripts_exec_output_per_sandbox_regardless_of_call_order() {
    let fake = FakeBackend::default()
        .with_exec_output_for("a", out("from a"))
        .with_exec_output_for("b", out("from b"))
        .with_default_exec_output(out("default"));

    assert_eq!(
        fake.exec("b", &spec(None, Stdin::Closed)).unwrap().stdout,
        "from b"
    );
    assert_eq!(
        fake.exec("a", &spec(None, Stdin::Closed)).unwrap().stdout,
        "from a"
    );
    // Persistent, not consumed; other sandboxes get the default.
    assert_eq!(
        fake.exec("a", &spec(None, Stdin::Closed)).unwrap().stdout,
        "from a"
    );
    assert_eq!(
        fake.exec("c", &spec(None, Stdin::Closed)).unwrap().stdout,
        "default"
    );
}

#[test]
fn fake_can_fail_create_or_exec_for_one_sandbox_only() {
    let fake = FakeBackend::default()
        .with_failing_create_for("bad")
        .with_failing_exec_for("worse");
    let create = |name: &str| {
        fake.create(&sbxm::backend::CreateSpec {
            name: name.into(),
            agent: "claude".into(),
            workspace: PathBuf::from("/w"),
            cpus: 1,
            memory: "1g".into(),
            skills: sbxm::backend::SkillsStore::Off,
            kits: vec![],
        })
    };

    assert!(create("bad").is_err());
    assert!(create("good").is_ok());
    // A failed create is still recorded.
    assert_eq!(fake.creates().len(), 2);
    assert!(fake.exec("worse", &spec(None, Stdin::Closed)).is_err());
    assert!(fake.exec("good", &spec(None, Stdin::Closed)).is_ok());
}

#[test]
fn the_exec_gate_blocks_execs_until_opened() {
    let (fake, gate) = FakeBackend::default().with_exec_gate();
    std::thread::scope(|s| {
        let handles: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|name| {
                let fake = &fake;
                s.spawn(move || fake.exec(name, &spec(None, Stdin::Closed)).unwrap())
            })
            .collect();

        gate.wait_for_blocked(2);
        // Both are inside exec and recorded, but neither has returned.
        assert_eq!(fake.execs().len(), 2);
        assert!(handles.iter().all(|h| !h.is_finished()));

        gate.open();
        for h in handles {
            h.join().unwrap();
        }
    });
}
