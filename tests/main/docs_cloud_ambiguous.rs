//! PR 322 review round 3, S-1: the README's cloud ownership contract names the gates and review
//! refusals of an ambiguous create, and a PR task's recovery (decision 268).

use std::fs;
use std::path::PathBuf;

fn readme_ambiguous_paragraph() -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let readme = fs::read_to_string(root.join("README.md")).expect("README.md is readable");
    let readme = readme.replace("\r\n", "\n");
    let start = readme
        .find("killed in between leaves")
        .expect("the README's cloud section describes an ambiguous create");
    let rest = &readme[start..];
    let end = rest.find("\n\n").expect("the paragraph ends");
    rest[..end].split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn the_readme_says_gates_review_and_fix_rounds_refuse_an_ambiguous_create() {
    let text = readme_ambiguous_paragraph();

    for path in ["`task gates`", "`task review`", "fix round"] {
        assert!(text.contains(path), "the paragraph doesn't name {path}");
    }
    assert!(
        text.contains("refuse before using its sandbox"),
        "the paragraph doesn't say they refuse before using the sandbox"
    );
}

#[test]
fn the_readme_gives_a_pr_tasks_recovery() {
    let text = readme_ambiguous_paragraph();

    assert!(
        text.contains("`task rm --pr N --forget-ambiguous`, then `task review --pr N`"),
        "the paragraph doesn't give the PR task's recovery"
    );
}
