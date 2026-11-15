//! Issue #416: the "List changed files" step of `.github/workflows/rust.yml` sets `build=false`
//! on a docs-only pull request, also when a `.md` path has non-ASCII characters or a quote (the
//! quoted display form `"docs/caf\303\251.md"` doesn't end in `.md`). The tests run the step's own
//! script, taken from the workflow file, in a temporary git repo, the way GitHub runs a bash step.
#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn workflow() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows/rust.yml");
    fs::read_to_string(path).unwrap().replace("\r\n", "\n")
}

/// The `run: |` block of the step named `name`, without its indentation.
fn step_script(workflow: &str, name: &str) -> String {
    let lines: Vec<&str> = workflow.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim() == format!("- name: {name}"))
        .unwrap_or_else(|| panic!("no step {name}"));
    let run = start
        + lines[start..]
            .iter()
            .position(|line| line.trim() == "run: |")
            .unwrap();
    let indent = lines[run + 1].len() - lines[run + 1].trim_start().len();
    lines[run + 1..]
        .iter()
        .take_while(|line| line.trim().is_empty() || line.len() - line.trim_start().len() >= indent)
        .map(|line| line.get(indent..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn git(repo: &Path, args: &[&str]) {
    let out = git_command(repo).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A git command with git's defaults (no user or system config, so `core.quotePath` is on, as on
/// a CI runner) and a fixed author.
fn git_command(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com");
    command
}

/// The result of running the step for `event` in a repo whose head commit adds `changed` on top
/// of a base commit (`commits` = 1 leaves only the base, so `HEAD^1` doesn't exist).
struct Step {
    out: Output,
    outputs: String,
}

fn run_step(event: &str, changed: &[&str], commits: u32) -> Step {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(repo.join("docs")).unwrap();
    git(&repo, &["init", "-q"]);
    fs::write(repo.join("README.md"), "base\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    if commits > 1 {
        for path in changed {
            let file = repo.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, "changed\n").unwrap();
        }
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "head"]);
    }
    let script =
        step_script(&workflow(), "List changed files").replace("${{ github.event_name }}", event);
    let script_file = tmp.path().join("step.sh");
    fs::write(&script_file, script).unwrap();
    let github_output = tmp.path().join("github-output");
    fs::write(&github_output, "").unwrap();
    // GitHub runs a `shell: bash` step as `bash --noprofile --norc -eo pipefail {0}`.
    let out = Command::new("bash")
        .args(["--noprofile", "--norc", "-eo", "pipefail"])
        .arg(&script_file)
        .current_dir(&repo)
        .env("GITHUB_OUTPUT", &github_output)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    let outputs = fs::read_to_string(&github_output).unwrap();
    Step { out, outputs }
}

fn pull_request(changed: &[&str]) -> Step {
    run_step("pull_request", changed, 2)
}

fn assert_build(step: &Step, build: bool) {
    assert!(step.out.status.success(), "{:?}", step.out);
    let docs_only = !build;
    assert!(
        step.outputs.contains(&format!("docs_only={docs_only}\n"))
            && step.outputs.contains(&format!("build={build}\n")),
        "outputs: {}\n{:?}",
        step.outputs,
        step.out
    );
}

#[test]
fn ci_docs_only_step_skips_the_build_for_a_non_ascii_md_path() {
    assert_build(&pull_request(&["docs/café.md"]), false);
}

#[test]
fn ci_docs_only_step_skips_the_build_for_an_md_path_with_a_quote() {
    assert_build(&pull_request(&["docs/with\"quote.md"]), false);
}

#[test]
fn ci_docs_only_step_skips_the_build_for_plain_md_paths() {
    assert_build(&pull_request(&["docs/a.md", "README.md"]), false);
}

#[test]
fn ci_docs_only_step_skips_the_build_for_an_md_path_with_a_space() {
    assert_build(&pull_request(&["docs/a b.md"]), false);
}

#[test]
fn ci_docs_only_step_builds_for_a_non_ascii_md_path_next_to_code() {
    assert_build(&pull_request(&["docs/café.md", "src/a.rs"]), true);
}

#[test]
fn ci_docs_only_step_builds_for_a_non_ascii_code_path() {
    assert_build(&pull_request(&["src/café.rs"]), true);
}

#[test]
fn ci_docs_only_step_builds_for_md_under_prompts_and_github() {
    assert_build(&pull_request(&["prompts/café.md"]), true);
    assert_build(&pull_request(&[".github/notes.md"]), true);
}

#[test]
fn ci_docs_only_step_builds_for_an_empty_diff() {
    assert_build(&pull_request(&[]), true);
}

#[test]
fn ci_docs_only_step_builds_for_a_push() {
    assert_build(&run_step("push", &["docs/café.md"], 2), true);
}

#[test]
fn ci_docs_only_step_fails_when_git_diff_fails() {
    // One commit: `HEAD^1` doesn't exist, so `git diff` fails and the step must fail too.
    let step = run_step("pull_request", &[], 1);
    assert!(!step.out.status.success(), "{:?}", step.out);
    assert!(!step.outputs.contains("build="), "{}", step.outputs);
}

/// The Ubuntu job has no docs-only skip: it builds and runs every test for any change.
#[test]
fn ci_docs_only_step_never_skips_the_ubuntu_build() {
    let workflow = workflow();
    let start = workflow.find("\n  build:\n").unwrap();
    let end = start + workflow[start..].find("\n  windows-shard:\n").unwrap();
    let job = &workflow[start..end];
    assert!(job.contains("ubuntu-latest"), "{job}");
    assert!(job.contains("cargo nextest run"), "{job}");
    assert!(!job.contains("if:"), "{job}");
}
