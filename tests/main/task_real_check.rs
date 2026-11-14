//! Issue 547 (#460 guardrail 3): `sbxm task real-check --pr N (--pass | --fail) "<note>"` posts
//! the commit status `sbxm/real-check` (`success` or `failure`) on the PR's current head commit,
//! with the note as its description, after checking the note (not empty, at most GitHub's 140
//! characters) and that the PR is open. The loop arms only when the head has `success`.

use std::path::Path;

use assert_cmd::Command;
use sbxm::commands::task_real_check::{self, Options};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{CommitStatus, GitHubBackend, PrInfo, PrState, StatusState};
use tempfile::TempDir;

const REPO: &str = "o/r";
const PR: u32 = 120;
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

fn pr(state: PrState) -> PrInfo {
    PrInfo {
        number: PR,
        head_ref: "sbxm/issue-20".into(),
        head_oid: HEAD.into(),
        is_cross_repository: false,
        head_repo: String::new(),
        state,
        title: "t".into(),
        body: String::new(),
        closing_issues: Vec::new(),
        base_moved: false,
    }
}

fn opts(pass: bool, note: &str) -> Options {
    Options {
        repo_root: std::env::temp_dir(),
        repo: Some(REPO.into()),
        pr: PR,
        pass,
        note: note.into(),
    }
}

fn run(gh: &FakeGitHub, opts: &Options) -> (anyhow::Result<()>, String) {
    let mut out = Vec::new();
    let result = task_real_check::run(opts, gh, &mut out);
    (result, String::from_utf8(out).unwrap())
}

fn posted(gh: &FakeGitHub) -> Vec<(String, CommitStatus)> {
    gh.calls()
        .into_iter()
        .filter_map(|call| match call {
            GhCall::CommitStatus(_, sha, status) => Some((sha, status)),
            _ => None,
        })
        .collect()
}

#[test]
fn pass_posts_success_on_the_prs_head() {
    let gh = FakeGitHub::default().with_pr(pr(PrState::Open));
    let (result, out) = run(&gh, &opts(true, "50 Windows runs green"));
    result.unwrap();
    assert_eq!(
        posted(&gh),
        [(
            HEAD.to_owned(),
            CommitStatus {
                state: StatusState::Success,
                context: "sbxm/real-check".into(),
                description: "50 Windows runs green".into(),
            }
        )]
    );
    assert!(out.contains("sbxm/real-check"), "{out}");
    assert!(out.contains(HEAD), "{out}");
    assert_eq!(
        gh.commit_status_state(REPO, HEAD, "sbxm/real-check")
            .unwrap()
            .as_deref(),
        Some("success")
    );
}

#[test]
fn fail_posts_failure() {
    let gh = FakeGitHub::default().with_pr(pr(PrState::Open));
    let (result, _) = run(&gh, &opts(false, "3 of 50 runs red"));
    result.unwrap();
    let got = posted(&gh);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].1.state, StatusState::Failure);
    assert_eq!(got[0].1.description, "3 of 50 runs red");
}

#[test]
fn an_empty_note_is_refused_before_github_is_read() {
    for note in ["", "   "] {
        let gh = FakeGitHub::default().with_pr(pr(PrState::Open));
        let (result, _) = run(&gh, &opts(true, note));
        let e = format!("{:#}", result.unwrap_err());
        assert!(e.contains("note"), "{e}");
        assert!(gh.calls().is_empty(), "{:?}", gh.calls());
    }
}

#[test]
fn a_note_of_140_characters_is_posted() {
    // Characters, not bytes: `é` is two bytes.
    let note = format!("{}é", "a".repeat(139));
    let gh = FakeGitHub::default().with_pr(pr(PrState::Open));
    let (result, _) = run(&gh, &opts(true, &note));
    result.unwrap();
    assert_eq!(posted(&gh)[0].1.description, note);
}

#[test]
fn a_note_of_141_characters_is_refused_saying_the_limit() {
    let gh = FakeGitHub::default().with_pr(pr(PrState::Open));
    let (result, _) = run(&gh, &opts(true, &"a".repeat(141)));
    let e = format!("{:#}", result.unwrap_err());
    assert!(e.contains("140"), "{e}");
    assert!(e.contains("141"), "{e}");
    assert!(gh.calls().is_empty(), "{:?}", gh.calls());
}

#[test]
fn a_closed_or_merged_pr_is_refused() {
    for (state, word) in [(PrState::Closed, "closed"), (PrState::Merged, "merged")] {
        let gh = FakeGitHub::default().with_pr(pr(state));
        let (result, _) = run(&gh, &opts(true, "ok"));
        let e = format!("{:#}", result.unwrap_err());
        assert!(e.contains(word), "{e}");
        assert!(posted(&gh).is_empty());
    }
}

#[test]
fn a_pr_that_cant_be_read_is_refused() {
    let gh = FakeGitHub::default();
    let (result, _) = run(&gh, &opts(true, "ok"));
    assert!(result.is_err());
    assert!(posted(&gh).is_empty());
}

#[test]
fn a_head_that_isnt_a_full_commit_id_is_refused() {
    let mut info = pr(PrState::Open);
    info.head_oid = "0123456".into();
    let gh = FakeGitHub::default().with_pr(info);
    let (result, _) = run(&gh, &opts(true, "ok"));
    let e = format!("{:#}", result.unwrap_err());
    assert!(e.contains("0123456"), "{e}");
    assert!(posted(&gh).is_empty());
}

#[test]
fn a_failed_post_is_an_error() {
    let gh = FakeGitHub::default()
        .with_pr(pr(PrState::Open))
        .failing_statuses("HTTP 403");
    let (result, out) = run(&gh, &opts(true, "ok"));
    let e = format!("{:#}", result.unwrap_err());
    assert!(e.contains("HTTP 403"), "{e}");
    assert!(!out.contains("posted"), "{out}");
}

// The command line.

fn cli(dir: &Path, args: &[&str]) -> String {
    let out = Command::cargo_bin("sbxm")
        .unwrap()
        .current_dir(dir)
        // The command must not need (or touch) the real config.
        .env("SBXM_CONFIG_DIR", dir.join("no-such-config"))
        .args(["task", "real-check"])
        .args(args)
        .assert()
        .failure()
        .get_output()
        .clone();
    String::from_utf8(out.stderr).unwrap()
}

#[test]
fn pass_and_fail_together_are_refused() {
    let dir = TempDir::new().unwrap();
    let stderr = cli(dir.path(), &["--pr", "1", "--pass", "--fail", "ok"]);
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

#[test]
fn neither_pass_nor_fail_is_refused() {
    let dir = TempDir::new().unwrap();
    let stderr = cli(dir.path(), &["--pr", "1", "ok"]);
    assert!(stderr.contains("required"), "{stderr}");
    assert!(stderr.contains("--pass"), "{stderr}");
}

#[test]
fn the_note_is_required() {
    let dir = TempDir::new().unwrap();
    let stderr = cli(dir.path(), &["--pr", "1", "--pass"]);
    assert!(stderr.contains("required"), "{stderr}");
}
