//! Issue 495: `task rm --merged` removes a spec task whose tip is reachable from `origin/<base>`
//! without asking for it to be held in the checkout too: the result is on origin, which is what
//! decision 178's guard protects. After a PR merge no ref in the checkout points at the tip (the
//! head branch is deleted, `main` points at the merge commit). The guard stays for a plain
//! `task rm --spec`.

use crate::common;

use std::path::PathBuf;

use common::task_fixture::{Fixture, Probe, backend, fixture, ready_spec_task};
use sbxm::commands::task_finish;
use sbxm::commands::task_rm::{Options, Target, run};
use sbxm::confirm::Confirm;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::merged::plan;
use sbxm::task::record;

struct Yes;

impl Confirm for Yes {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        Ok(true)
    }
}

/// The `sbxm-task.toml` checkout: it never fetches a task's branch, so no ref there points at a
/// spec task's tip.
fn repo_root(f: &Fixture) -> PathBuf {
    f.env.tmp.path().join("target-repo")
}

/// A spec task finished through the `local` sink: its branch is only in its own `repo.git`.
fn kept_spec_task(f: &Fixture) -> (PathBuf, String) {
    let (spec, id) = ready_spec_task(f);
    task_finish::run(
        &f.env.config_dir(),
        &task_finish::Options {
            target: task_finish::Target::Spec(spec.clone()),
            repo_root: repo_root(f),
            push_unresolved: false,
        },
        &sbxm::backend::FakeBackend::default(),
        &FakeGitHub::default(),
        &Probe,
        &mut Vec::new(),
    )
    .unwrap();
    (spec, id)
}

/// How `id`'s branch lands on `main` on `f.origin`.
enum Landing {
    /// A fast-forward push (a `push` sink branch merged without a merge commit).
    FastForward,
    /// A merge commit, `main` having moved on first (GitHub's "Create a merge commit").
    MergeCommit,
    /// One new commit with the branch's changes (GitHub's "Squash and merge").
    Squash,
}

/// Lands `id`'s branch (from its `repo.git`) on `main` on `f.origin` the way `landing` says,
/// leaving no branch named `id` anywhere but in the task's own `repo.git`.
fn land_on_origin_main(f: &Fixture, id: &str, landing: Landing) {
    let repo_git = record::task_dir(&f.env.base_dir(), id).join("repo.git");
    let scratch = f.env.tmp.path().join("merge-scratch");
    common::git(
        f.env.tmp.path(),
        &[
            "clone",
            "-q",
            f.origin.to_str().unwrap(),
            scratch.to_str().unwrap(),
        ],
    );
    common::git(
        &scratch,
        &[
            "fetch",
            "-q",
            repo_git.to_str().unwrap(),
            &format!("{id}:{id}"),
        ],
    );
    match landing {
        Landing::FastForward => {
            common::git(&scratch, &["merge", "--ff-only", "-q", id]);
        }
        Landing::MergeCommit => {
            std::fs::write(scratch.join("other.txt"), "other\n").unwrap();
            common::git(&scratch, &["add", "other.txt"]);
            common::git(&scratch, &["commit", "-q", "-m", "other work on main"]);
            common::git(&scratch, &["merge", "--no-ff", "-q", "-m", "Merge PR", id]);
        }
        Landing::Squash => {
            common::git(&scratch, &["merge", "--squash", "-q", id]);
            common::git(&scratch, &["commit", "-q", "-m", "Squashed PR"]);
        }
    }
    common::git(&scratch, &["push", "-q", "origin", "main"]);
}

fn assert_candidate(f: &Fixture, id: &str) {
    let found = plan(&f.env.base_dir(), &FakeGitHub::default(), &Probe).unwrap();

    assert_eq!(found.candidates.len(), 1, "{found:?}");
    assert_eq!(found.candidates[0].id, id);
    assert!(
        found.candidates[0].reason.contains("merged"),
        "{}",
        found.candidates[0].reason
    );
    assert!(found.left_alone.is_empty(), "{found:?}");
}

fn assert_left_alone_as_not_merged(f: &Fixture, id: &str) {
    let found = plan(&f.env.base_dir(), &FakeGitHub::default(), &Probe).unwrap();

    assert!(found.candidates.is_empty(), "{found:?}");
    assert_eq!(found.left_alone.len(), 1, "{found:?}");
    assert_eq!(found.left_alone[0].id, id);
    let reason = &found.left_alone[0].reason;
    assert!(reason.contains("not merged"), "{reason}");
    assert!(!reason.contains("git fetch"), "{reason}");
}

#[test]
fn a_spec_task_merged_with_a_merge_commit_and_no_local_ref_at_its_tip_is_a_candidate() {
    let f = fixture();
    let (_spec, id) = kept_spec_task(&f);
    land_on_origin_main(&f, &id, Landing::MergeCommit);

    assert_candidate(&f, &id);
}

#[test]
fn a_spec_task_fast_forwarded_onto_origin_base_and_never_fetched_is_a_candidate() {
    let f = fixture();
    let (_spec, id) = kept_spec_task(&f);
    land_on_origin_main(&f, &id, Landing::FastForward);

    assert_candidate(&f, &id);
}

#[test]
fn a_squash_merged_spec_task_is_still_left_alone_as_not_merged() {
    let f = fixture();
    let (_spec, id) = kept_spec_task(&f);
    land_on_origin_main(&f, &id, Landing::Squash);

    assert_left_alone_as_not_merged(&f, &id);
}

#[test]
fn an_unmerged_and_unfetched_spec_task_is_still_left_alone_as_not_merged() {
    let f = fixture();
    let (_spec, id) = kept_spec_task(&f);

    assert_left_alone_as_not_merged(&f, &id);
}

fn rm(f: &Fixture, target: Target) -> (anyhow::Result<()>, String) {
    let mut out = Vec::new();
    let result = run(
        &f.env.config_dir(),
        &Options {
            target,
            yes: true,
            force: false,
            repo_root: repo_root(f),
            dry_run: false,
            forget_ambiguous: false,
        },
        &backend(),
        &FakeGitHub::default(),
        &Probe,
        &Yes,
        &mut out,
    );
    (result, String::from_utf8(out).unwrap())
}

fn task_dir(f: &Fixture, id: &str) -> PathBuf {
    record::task_dir(&f.env.base_dir(), id)
}

#[test]
fn rm_merged_removes_a_spec_task_merged_with_a_merge_commit() {
    let f = fixture();
    let (_spec, id) = kept_spec_task(&f);
    land_on_origin_main(&f, &id, Landing::MergeCommit);

    let (result, out) = rm(&f, Target::Merged);

    result.unwrap();
    assert!(!task_dir(&f, &id).exists(), "{out}");
    assert!(!out.contains("left alone"), "{out}");
}

/// The guard stays for a plain `task rm --spec` (issue 143 unchanged), even when the tip is on
/// `origin/<base>`: only `--merged` proves that first.
#[test]
fn rm_spec_still_refuses_a_merged_but_unfetched_result_without_force() {
    let f = fixture();
    let (spec, id) = kept_spec_task(&f);
    land_on_origin_main(&f, &id, Landing::MergeCommit);

    let (result, _) = rm(&f, Target::Spec(spec));

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("git fetch"), "{message}");
    assert!(message.contains("--force"), "{message}");
    assert!(task_dir(&f, &id).join("repo.git").is_dir());
}
