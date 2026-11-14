//! Issue 244, S-1: the trust-boundary paragraphs in README.md and docs/workflow.md must disclose
//! that a worker's commits are rewritten (message neutralised, signature dropped) before they
//! reach the delivered branch -- not just that they are collected through a verified bundle.

use std::fs;

#[test]
fn the_readme_trust_boundary_discloses_the_commit_rewrite() {
    let readme = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap();

    assert!(
        readme.contains("rewrit"),
        "README.md's trust-boundary paragraph should say worker commits are rewritten"
    );
    assert!(
        readme.contains("unsigned") || readme.contains("signature"),
        "README.md's trust-boundary paragraph should say rebuilt commits carry no signature"
    );
}

#[test]
fn the_workflow_doc_trust_boundary_discloses_the_commit_rewrite() {
    let workflow =
        fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/workflow.md")).unwrap();

    assert!(
        workflow.contains("rewrit"),
        "docs/workflow.md's trust-boundary paragraph should say worker commits are rewritten"
    );
    assert!(
        workflow.contains("unsigned") || workflow.contains("signature"),
        "docs/workflow.md's trust-boundary paragraph should say rebuilt commits carry no signature"
    );
}

/// PR 250 review round 3, S-1 (decision 198): both user-facing trust-boundary paragraphs must say
/// a commit over 1 MiB is refused instead of rewritten.
#[test]
fn both_trust_boundary_paragraphs_disclose_the_commit_size_cap() {
    for doc in ["README.md", "docs/workflow.md"] {
        let text = fs::read_to_string(format!("{}/{doc}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let paragraph = text
            .split("\n\n")
            .find(|p| p.contains("rewrites every one of those commits"))
            .unwrap_or_else(|| panic!("{doc} has no trust-boundary paragraph about the rewrite"));
        assert!(
            paragraph.contains("1 MiB") && paragraph.contains("refused"),
            "{doc}'s trust-boundary paragraph should say a commit over 1 MiB is refused"
        );
    }
}
