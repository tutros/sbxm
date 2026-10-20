//! Milestone 3 slice 8: `sbxm task`'s sandboxes follow the profile's template (decisions 200, 203,
//! 206, 216).

mod common;

use std::fs;

use common::{Env, git};
use sbxm::template::Locks;

#[test]
fn lock_files_at_a_commit_hash_like_the_same_files_in_a_folder() {
    let env = Env::new();
    let repo = env.tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    fs::write(repo.join("Cargo.lock"), "version = 4\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "lock"]);
    let first = git(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("Cargo.lock"), "version = 4\n# changed\n").unwrap();
    git(&repo, &["commit", "-am", "changed lock"]);
    let files = ["Cargo.lock".to_owned()];
    let repo_git = repo.join(".git");

    let at_head = Locks::Commit {
        repo_git: &repo_git,
        commit: &git(&repo, &["rev-parse", "HEAD"]),
    }
    .hash(&files)
    .unwrap();
    let at_first = Locks::Commit {
        repo_git: &repo_git,
        commit: &first,
    }
    .hash(&files)
    .unwrap();

    assert_eq!(at_head, Locks::Dir(&repo).hash(&files).unwrap());
    assert_ne!(at_first, at_head);
    let missing = Locks::Commit {
        repo_git: &repo_git,
        commit: &first,
    }
    .hash(&["Cargo.toml".to_owned()]);
    assert!(missing.is_err());
}
