//! Issue 549 (#460 guardrail 6): the reasons a medium-risk PR isn't armed, besides its risk: its
//! issue's size (S or M only, read from its labels at arming time; a spec job counts as S) and a
//! change to any `Cargo.lock`, from the PR's file list.

mod common;

use common::task_fixture::issue_text;
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{IssueText, PrFile, PrFiles};
use sbxm::task::medium::{self, Source};

const REPO: &str = "o/r";
const PR: u32 = 120;
const ISSUE: u32 = 20;

fn labeled(labels: &str) -> IssueText {
    let mut text = issue_text(ISSUE);
    text.text = format!(
        "title:\tFix {ISSUE}\nstate:\tOPEN\nauthor:\tme\nlabels:\t{labels}\ncomments:\t0\n--\nbody\n"
    );
    text
}

fn file(name: &str) -> PrFile {
    PrFile {
        filename: name.to_owned(),
        previous_filename: None,
    }
}

fn renamed(from: &str, to: &str) -> PrFile {
    PrFile {
        filename: to.to_owned(),
        previous_filename: Some(from.to_owned()),
    }
}

fn files(list: Vec<PrFile>) -> PrFiles {
    PrFiles {
        changed_files: u32::try_from(list.len()).unwrap(),
        files: list,
    }
}

fn plain() -> PrFiles {
    files(vec![file("src/main.rs")])
}

fn reasons(gh: &FakeGitHub, source: Source) -> Vec<String> {
    medium::refusals(gh, REPO, source, PR)
}

fn sized(labels: &str) -> Vec<String> {
    let gh = FakeGitHub::default()
        .with_issue_text(labeled(labels))
        .with_pr_files(PR, plain());
    reasons(&gh, Source::Issue(ISSUE))
}

fn changing(list: PrFiles) -> Vec<String> {
    let gh = FakeGitHub::default()
        .with_issue_text(labeled("ready-for-worker, size:S"))
        .with_pr_files(PR, list);
    reasons(&gh, Source::Issue(ISSUE))
}

// The size.

#[test]
fn size_s_passes() {
    assert_eq!(sized("ready-for-worker, size:S"), Vec::<String>::new());
}

#[test]
fn size_m_passes() {
    assert_eq!(sized("size:M"), Vec::<String>::new());
}

#[test]
fn size_l_is_refused() {
    assert_eq!(
        sized("size:L, ready-for-worker"),
        ["its issue #20 is size L"]
    );
}

#[test]
fn no_size_label_is_refused() {
    assert_eq!(
        sized("ready-for-worker"),
        ["its issue #20 has no size label"]
    );
}

#[test]
fn no_labels_at_all_is_refused() {
    let gh = FakeGitHub::default()
        .with_issue_text(issue_text(ISSUE))
        .with_pr_files(PR, plain());
    assert_eq!(
        reasons(&gh, Source::Issue(ISSUE)),
        ["its issue #20 has no size label"]
    );
}

#[test]
fn two_size_labels_are_refused() {
    assert_eq!(
        sized("size:S, size:M"),
        ["its issue #20 has more than one size label (S, M)"]
    );
}

#[test]
fn a_spec_jobs_pr_counts_as_size_s() {
    let gh = FakeGitHub::default().with_pr_files(PR, plain());
    assert_eq!(reasons(&gh, Source::Spec), Vec::<String>::new());
}

#[test]
fn an_issue_whose_labels_cant_be_read_is_refused() {
    let gh = FakeGitHub::default().with_pr_files(PR, plain());
    let got = reasons(&gh, Source::Issue(ISSUE));
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0].starts_with("its issue #20's labels could not be read: "),
        "{got:?}"
    );
}

#[test]
fn a_pr_whose_source_is_unknown_is_refused() {
    let gh = FakeGitHub::default().with_pr_files(PR, plain());
    assert_eq!(
        reasons(&gh, Source::Unknown),
        ["the issue it came from is unknown"]
    );
}

// Cargo.lock.

#[test]
fn cargo_lock_at_the_root_is_refused() {
    assert_eq!(
        changing(files(vec![file("src/a.rs"), file("Cargo.lock")])),
        ["it changes Cargo.lock"]
    );
}

#[test]
fn cargo_lock_in_a_subdirectory_is_refused() {
    assert_eq!(
        changing(files(vec![file("crates/x/Cargo.lock")])),
        ["it changes crates/x/Cargo.lock"]
    );
}

#[test]
fn a_deleted_cargo_lock_is_refused() {
    // GitHub lists a deleted file under its own name.
    assert_eq!(
        changing(files(vec![file("Cargo.lock")])),
        ["it changes Cargo.lock"]
    );
}

#[test]
fn a_rename_from_cargo_lock_is_refused() {
    assert_eq!(
        changing(files(vec![renamed("Cargo.lock", "Cargo.lock.old")])),
        ["it changes Cargo.lock"]
    );
}

#[test]
fn a_rename_to_cargo_lock_is_refused() {
    assert_eq!(
        changing(files(vec![renamed("old/lockfile", "Cargo.lock")])),
        ["it changes Cargo.lock"]
    );
}

#[test]
fn cargo_lock_bak_and_cargo_toml_pass() {
    assert_eq!(
        changing(files(vec![
            file("Cargo.lock.bak"),
            file("Cargo.toml"),
            file("x/Cargo.lockfile"),
            file("x/my-Cargo.lock"),
        ])),
        Vec::<String>::new()
    );
}

// The file list.

fn many(n: usize) -> Vec<PrFile> {
    (0..n).map(|i| file(&format!("src/f{i}.rs"))).collect()
}

#[test]
fn a_list_of_100_files_passes() {
    assert_eq!(changing(files(many(100))), Vec::<String>::new());
}

#[test]
fn a_list_of_101_files_passes() {
    assert_eq!(changing(files(many(101))), Vec::<String>::new());
}

#[test]
fn cargo_lock_only_in_the_last_page_is_refused() {
    let mut list = many(100);
    list.push(file("Cargo.lock"));
    assert_eq!(changing(files(list)), ["it changes Cargo.lock"]);
}

#[test]
fn a_list_shorter_than_changed_files_is_refused() {
    let mut list = files(many(100));
    list.changed_files = 101;
    assert_eq!(
        changing(list),
        ["its file list doesn't match its changed-files count (100 listed, 101 changed)"]
    );
}

#[test]
fn a_list_that_cant_be_read_is_refused() {
    let gh = FakeGitHub::default().with_issue_text(labeled("size:S"));
    let got = reasons(&gh, Source::Issue(ISSUE));
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0].starts_with("its file list could not be read: "),
        "{got:?}"
    );
}

#[test]
fn every_reason_is_given_together() {
    let gh = FakeGitHub::default()
        .with_issue_text(labeled("size:L"))
        .with_pr_files(PR, files(vec![file("Cargo.lock")]));
    assert_eq!(
        reasons(&gh, Source::Issue(ISSUE)),
        ["its issue #20 is size L", "it changes Cargo.lock"]
    );
}
