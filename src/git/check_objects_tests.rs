//! Issue #444: `check_objects` answers many object or ref lookups with one git start.

use super::*;
use std::process::Stdio;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A bare repo with a chain of `count` commits on `refs/heads/main`; returns the ids oldest first.
fn chain(count: usize) -> (tempfile::TempDir, std::path::PathBuf, Vec<String>) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo.git");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main", "--bare"]);
    let tree = git(&dir, &["hash-object", "-t", "tree", "--stdin", "-w"]);
    let mut ids: Vec<String> = Vec::new();
    for n in 0..count {
        let message = format!("commit {n}");
        let mut args = vec!["commit-tree", &tree, "-m", &message];
        if let Some(parent) = ids.last() {
            args.extend(["-p", parent]);
        }
        let id = git(&dir, &args);
        ids.push(id);
    }
    if let Some(tip) = ids.last() {
        git(&dir, &["update-ref", "refs/heads/main", tip]);
    }
    (tmp, dir, ids)
}

fn check(dir: &Path, names: &[&str]) -> Result<Vec<Option<Object>>> {
    check_objects(dir, Some(dir), None, names)
}

#[test]
fn a_batch_of_one_ref_resolves_to_its_commit() {
    let (_tmp, dir, ids) = chain(1);

    let found = check(&dir, &["refs/heads/main^{commit}"]).unwrap();

    let object = found[0].as_ref().unwrap();
    assert_eq!(object.id, ids[0]);
    assert_eq!(object.kind, "commit");
    assert!(object.size > 0);
}

#[test]
fn a_batch_of_fifty_commits_answers_in_order() {
    let (_tmp, dir, ids) = chain(50);
    let names: Vec<&str> = ids.iter().map(String::as_str).collect();

    let found = check(&dir, &names).unwrap();

    assert_eq!(found.len(), 50);
    for (answer, id) in found.iter().zip(&ids) {
        assert_eq!(&answer.as_ref().unwrap().id, id);
    }
}

#[test]
fn a_missing_ref_is_none_and_the_others_still_resolve() {
    let (_tmp, dir, ids) = chain(2);

    let found = check(
        &dir,
        &[
            "refs/heads/main^{commit}",
            "refs/heads/nope^{commit}",
            &ids[0],
        ],
    )
    .unwrap();

    assert_eq!(found[0].as_ref().unwrap().id, ids[1]);
    assert!(found[1].is_none());
    assert_eq!(found[2].as_ref().unwrap().id, ids[0]);
}

#[test]
fn a_batch_where_every_ref_is_missing_is_all_none() {
    let (_tmp, dir, _ids) = chain(1);

    let found = check(&dir, &["refs/heads/a", "refs/heads/b"]).unwrap();

    assert_eq!(found.len(), 2);
    assert!(found.iter().all(Option::is_none));
}

#[test]
fn an_empty_batch_starts_no_git_at_all() {
    // A directory that is no repository and a `git_dir` that doesn't exist: any git start would
    // fail, so only an early return gives Ok.
    let missing = Path::new("/nonexistent/for/check_objects");

    let found = check_objects(Path::new("."), Some(missing), None, &[]).unwrap();

    assert!(found.is_empty());
}

#[test]
fn a_name_that_looks_like_an_option_is_a_name_not_an_option() {
    let (_tmp, dir, ids) = chain(1);

    let found = check(&dir, &["-x", "--help", &ids[0]]).unwrap();

    assert!(found[0].is_none());
    assert!(found[1].is_none());
    assert_eq!(found[2].as_ref().unwrap().id, ids[0]);
}

#[test]
fn a_name_with_a_newline_is_refused_before_git_starts() {
    let (_tmp, dir, ids) = chain(1);

    let err = check(&dir, &[&ids[0], "refs/heads/a\n{ids}"]).unwrap_err();

    assert!(format!("{err:#}").contains("newline"), "{err:#}");
}

#[test]
fn an_empty_name_is_refused() {
    let (_tmp, dir, _ids) = chain(1);

    let err = check(&dir, &["refs/heads/main", ""]).unwrap_err();

    assert!(format!("{err:#}").contains("empty"), "{err:#}");
}

#[test]
fn a_tree_name_peels_to_the_tree_and_reports_its_kind() {
    let (_tmp, dir, ids) = chain(1);

    let found = check(
        &dir,
        &[
            &format!("{}^{{tree}}", ids[0]),
            &format!("{}^{{commit}}", ids[0]),
        ],
    )
    .unwrap();

    assert_eq!(found[0].as_ref().unwrap().kind, "tree");
    assert_eq!(found[1].as_ref().unwrap().kind, "commit");
}

#[test]
fn read_objects_returns_each_object_body_in_order() {
    let (_tmp, dir, ids) = chain(3);
    let names: Vec<&str> = ids.iter().map(String::as_str).collect();

    let bodies = read_objects(&dir, Some(&dir), None, &names).unwrap();

    assert_eq!(bodies.len(), 3);
    for (n, body) in bodies.iter().enumerate() {
        let text = String::from_utf8(body.clone()).unwrap();
        assert!(text.starts_with("tree "), "{text}");
        assert!(text.ends_with(&format!("commit {n}\n")), "{text:?}");
        assert_eq!(
            body.as_slice(),
            git_bytes(&dir, &["cat-file", "commit", &ids[n]]).as_slice()
        );
    }
}

#[test]
fn read_objects_keeps_bytes_that_are_not_utf8() {
    let (_tmp, dir, _ids) = chain(1);
    let blob = Command::new("git")
        .current_dir(&dir)
        .args(["hash-object", "-w", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"a\n\n\xff\xfe\nb\n")?;
            child.wait_with_output()
        })
        .unwrap();
    let id = String::from_utf8(blob.stdout).unwrap().trim().to_owned();

    let bodies = read_objects(&dir, Some(&dir), None, &[&id, &id]).unwrap();

    assert_eq!(bodies[0], b"a\n\n\xff\xfe\nb\n");
    assert_eq!(bodies[1], b"a\n\n\xff\xfe\nb\n");
}

#[test]
fn read_objects_fails_when_one_object_is_missing() {
    let (_tmp, dir, ids) = chain(1);

    let err = read_objects(&dir, Some(&dir), None, &[&ids[0], "refs/heads/nope"]).unwrap_err();

    assert!(format!("{err:#}").contains("refs/heads/nope"), "{err:#}");
}

#[test]
fn read_objects_with_no_names_starts_no_git() {
    let missing = Path::new("/nonexistent/for/read_objects");

    assert!(
        read_objects(Path::new("."), Some(missing), None, &[])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn read_objects_refuses_a_name_with_a_newline() {
    let (_tmp, dir, ids) = chain(1);

    let err = read_objects(&dir, Some(&dir), None, &[&ids[0], "a\nb"]).unwrap_err();

    assert!(format!("{err:#}").contains("newline"), "{err:#}");
}

fn git_bytes(dir: &Path, args: &[&str]) -> Vec<u8> {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
        .stdout
}
