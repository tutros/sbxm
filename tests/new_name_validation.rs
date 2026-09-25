use assert_cmd::Command;
use tempfile::TempDir;

/// Runs `sbxm new <name>` with an empty temp dir as both config dir and
/// working dir, and returns stderr, exit success, and whether the dir is
/// still empty.
fn run_new(name: &str) -> (String, bool, bool) {
    let tmp = TempDir::new().unwrap();
    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .current_dir(tmp.path())
        .env("SBXM_CONFIG_DIR", tmp.path().join("config"))
        .args(["new", "--", name])
        .output()
        .unwrap();
    let untouched = std::fs::read_dir(tmp.path()).unwrap().next().is_none();
    (
        String::from_utf8(output.stderr).unwrap(),
        output.status.success(),
        untouched,
    )
}

fn assert_rejected(name: &str, reason: &str) {
    let (stderr, success, untouched) = run_new(name);
    assert!(!success, "{name:?} was accepted");
    assert!(
        stderr.contains(&format!("invalid project name '{name}'")),
        "{name:?}: stderr: {stderr}"
    );
    assert!(stderr.contains(reason), "{name:?}: stderr: {stderr}");
    assert!(untouched, "{name:?}: files were created");
}

fn assert_valid(name: &str) {
    let (stderr, _, _) = run_new(name);
    assert!(
        !stderr.contains("invalid project name"),
        "{name:?} was rejected: {stderr}"
    );
}

#[test]
fn accepts_valid_names() {
    for name in ["demo", r"ab", "my-project-2"] {
        assert_valid(name);
    }
}

#[test]
fn rejects_characters_outside_lowercase_digits_and_hyphen() {
    for name in ["Demo", "a/b", r"a\b", "..", ".hidden", "my_project"] {
        assert_rejected(name, "only lowercase letters, digits and '-'");
    }
}
