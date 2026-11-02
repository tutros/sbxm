//! Issue 313: which review `sbxm task fix --pr N` fixes. Only the latest comment by the
//! authenticated user that carries sbxm's review header counts, its `Scope:` range must end at
//! the PR's head, and its must-fix findings (only those) are the worker's input. Every refusal
//! says what was found and why it stops.

use sbxm::github::{PrComment, PrInfo, PrState};
use sbxm::task::pr_fix::review_to_fix;
use sbxm::task::review::{in_task_pr_comment, pr_comment};

const HEAD: &str = "1234567890abcdef1234567890abcdef12345678";
const OLD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn pr() -> PrInfo {
    PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        head_oid: HEAD.into(),
        is_cross_repository: false,
        head_repo: "o/r".into(),
        state: PrState::Open,
        title: "Add the x feature".into(),
        body: "This adds x.".into(),
        closing_issues: vec![],
    }
}

fn review(head: &str, must_fix: u32) -> String {
    let mut text = format!(
        "Reviewer: codex (default model)\n\nMust-fix findings: {must_fix}\nRisk: medium\n\n\
         Scope: `0000000..{head}`\nIssues: pending\n\n"
    );
    if must_fix > 0 {
        text.push_str("## Must fix\n\n");
        for n in 1..=must_fix {
            text.push_str(&format!(
                "### M-{n} - The lock number {n} is never released\n\n\
                 **Where:** `src/lock.rs:{n}`\n\
                 **What happens:** it stays held\n\
                 **Why it matters:** decision 1\n\
                 **Fix:** release it\n\n"
            ));
        }
    }
    text.push_str(
        "## Should fix\n\n### S-1 - A name is unclear\n\n**Where:** `src/name.rs:1`\n\
         **What happens:** x\n**Why it matters:** y\n**Fix:** rename it\n",
    );
    text
}

fn by(author: &str, body: String) -> PrComment {
    PrComment {
        author: author.into(),
        body,
    }
}

/// A review as `task review --pr` posts it.
fn posted(head: &str, must_fix: u32) -> String {
    pr_comment(7, "", &review(head, must_fix))
}

fn refusal(pr: &PrInfo, comments: &[PrComment]) -> String {
    format!(
        "{:#}",
        review_to_fix(pr, "o/r", None, "tutros", comments).unwrap_err()
    )
}

#[test]
fn the_latest_review_by_the_user_covering_the_head_gives_its_must_fix_findings_only() {
    let comments = [
        by("tutros", posted(OLD, 2)),
        by("tutros", "a plain comment".into()),
        by("tutros", posted(HEAD, 1)),
    ];

    let chosen = review_to_fix(&pr(), "o/r", None, "tutros", &comments).unwrap();

    assert_eq!(chosen.commit, HEAD);
    assert_eq!(chosen.must_fix, 1);
    assert!(
        chosen
            .text
            .contains("### M-1 - The lock number 1 is never released"),
        "{}",
        chosen.text
    );
    assert!(
        chosen.text.contains("**Fix:** release it"),
        "{}",
        chosen.text
    );
    assert!(!chosen.text.contains("S-1"), "{}", chosen.text);
    assert!(
        !chosen.text.contains("A name is unclear"),
        "{}",
        chosen.text
    );
    assert!(chosen.text.contains("PR #7"), "{}", chosen.text);
}

#[test]
fn an_in_task_review_posted_by_task_finish_counts_too() {
    let comments = [by(
        "tutros",
        in_task_pr_comment(41, HEAD, "", &review(HEAD, 1)),
    )];
    let chosen = review_to_fix(&pr(), "o/r", None, "tutros", &comments).unwrap();
    assert_eq!(chosen.commit, HEAD);
}

#[test]
fn a_scope_with_a_short_sha_of_the_head_counts() {
    let comments = [by("tutros", posted(&HEAD[..7], 1))];
    let chosen = review_to_fix(&pr(), "o/r", None, "tutros", &comments).unwrap();
    assert_eq!(chosen.commit, HEAD);
}

#[test]
fn a_review_with_no_must_fix_is_refused() {
    let message = refusal(&pr(), &[by("tutros", posted(HEAD, 0))]);
    assert!(
        message.contains(&format!(
            "the latest sbxm review of PR #7 (commit {HEAD}) has no must-fix findings, so there \
             is nothing to fix; file its other findings with `sbxm task file-findings`"
        )),
        "{message}"
    );
}

#[test]
fn a_head_that_moved_after_the_review_is_refused() {
    let message = refusal(&pr(), &[by("tutros", posted(OLD, 1))]);
    assert!(
        message.contains(&format!(
            "the latest sbxm review covers {OLD}, but PR #7's head is now {HEAD}; review the new \
             head first with `sbxm task review --pr 7 --repo o/r`"
        )),
        "{message}"
    );
}

#[test]
fn a_head_one_commit_past_a_short_sha_is_refused_too() {
    let message = refusal(&pr(), &[by("tutros", posted(&OLD[..7], 1))]);
    assert!(message.contains("covers aaaaaaa, but"), "{message}");
}

#[test]
fn a_review_posted_by_someone_else_is_ignored() {
    // The latest comment carrying the header is someone else's; the user's own is older and
    // covers the head, so it is the one used.
    let comments = [
        by("tutros", posted(HEAD, 1)),
        by("mallory", posted(HEAD, 3)),
    ];
    let chosen = review_to_fix(&pr(), "o/r", None, "tutros", &comments).unwrap();
    assert_eq!(chosen.must_fix, 1);
}

#[test]
fn only_someone_elses_review_is_refused_as_no_review() {
    let message = refusal(&pr(), &[by("mallory", posted(HEAD, 1))]);
    assert!(
        message.contains(
            "PR #7 has no sbxm review by tutros; run `sbxm task review --pr 7 --repo o/r` first"
        ),
        "{message}"
    );
}

#[test]
fn no_review_at_all_is_refused() {
    let message = refusal(&pr(), &[by("tutros", review(HEAD, 1))]);
    assert!(
        message.contains("PR #7 has no sbxm review by tutros; run"),
        "{message}"
    );
}

#[test]
fn a_fork_pr_is_refused_naming_the_fork() {
    let mut fork = pr();
    fork.is_cross_repository = true;
    fork.head_repo = "someone/r".into();
    let message = refusal(&fork, &[by("tutros", posted(HEAD, 1))]);
    assert!(
        message.contains(
            "PR #7 comes from a fork (someone/r); sbxm can push only to this repo's own \
             branches; `sbxm task review --pr 7 --repo o/r` still works"
        ),
        "{message}"
    );
}

#[test]
fn closed_and_merged_prs_are_refused() {
    for (state, word) in [(PrState::Closed, "closed"), (PrState::Merged, "merged")] {
        let mut done = pr();
        done.state = state;
        let message = refusal(&done, &[by("tutros", posted(HEAD, 1))]);
        assert!(
            message.contains(&format!(
                "PR #7 is {word}; there is no open branch to push to"
            )),
            "{message}"
        );
    }
}

#[test]
fn a_review_without_a_scope_range_is_refused() {
    let body = pr_comment(
        7,
        "",
        "Must-fix findings: 1\n\n## Must fix\n\n### M-1 - x\n",
    );
    let message = refusal(&pr(), &[by("tutros", body)]);
    assert!(
        message.contains("the latest sbxm review of PR #7 has no `Scope:` range"),
        "{message}"
    );
}

#[test]
fn a_must_fix_count_with_no_readable_findings_is_refused() {
    let body = pr_comment(
        7,
        "",
        &format!("Must-fix findings: 2\n\nScope: `0000000..{HEAD}`\n\nTwo things are wrong.\n"),
    );
    let message = refusal(&pr(), &[by("tutros", body)]);
    assert!(
        message.contains("says it has 2 must-fix finding(s), but none could be read"),
        "{message}"
    );
}

#[test]
fn the_base_is_named_in_the_review_hint_when_given() {
    let message = format!(
        "{:#}",
        review_to_fix(&pr(), "o/r", Some("dev"), "tutros", &[]).unwrap_err()
    );
    assert!(
        message.contains("`sbxm task review --pr 7 --repo o/r --base dev`"),
        "{message}"
    );
}

/// `review(HEAD, must_fix)` with `pad` characters added to M-`at`'s **What happens:**, so a
/// posted comment is cut where the caller wants it.
fn padded(must_fix: u32, at: u32, pad: usize) -> String {
    let marker = format!("`src/lock.rs:{at}`\n**What happens:** it stays held");
    let text = review(HEAD, must_fix);
    assert!(text.contains(&marker));
    text.replace(&marker, &format!("{marker} {}", "x".repeat(pad)))
}

/// Review M-2: a review cut to fit GitHub before a later must-fix finding would give the worker
/// only part of its input.
#[test]
fn a_review_cut_before_a_later_must_fix_finding_is_refused() {
    let body = pr_comment(7, "", &padded(2, 1, 60_000));
    assert!(body.contains("sbxm cut this review"));
    assert!(!body.contains("### M-2"));

    let message = refusal(&pr(), &[by("tutros", body)]);

    assert!(
        message.contains(&format!(
            "the latest sbxm review of PR #7 (commit {HEAD}) was cut to fit a GitHub comment, so \
             not all of its must-fix findings are on the PR; review the head again with \
             `sbxm task review --pr 7 --repo o/r`"
        )),
        "{message}"
    );
}

/// Review M-2: a cut inside the last must-fix finding is refused too, though the count matches.
#[test]
fn a_review_cut_inside_its_last_must_fix_finding_is_refused() {
    let body = pr_comment(7, "", &padded(1, 1, 60_000));
    assert!(body.contains("sbxm cut this review"));
    assert!(body.contains("### M-1"));

    let message = refusal(&pr(), &[by("tutros", body)]);

    assert!(
        message.contains("was cut to fit a GitHub comment"),
        "{message}"
    );
}

/// Review M-2: the stated count must match the findings read, not only when none were.
#[test]
fn a_must_fix_count_above_the_findings_read_is_refused() {
    let text = review(HEAD, 2).replace("### M-2 - ", "M-2 - ");
    let body = pr_comment(7, "", &text);

    let message = refusal(&pr(), &[by("tutros", body)]);

    assert!(
        message.contains(&format!(
            "the latest sbxm review of PR #7 (commit {HEAD}) says it has 2 must-fix finding(s), \
             but 1 could be read"
        )),
        "{message}"
    );
}

#[test]
fn a_must_fix_count_below_the_findings_read_is_refused() {
    let text = review(HEAD, 2).replace("Must-fix findings: 2", "Must-fix findings: 1");
    let body = pr_comment(7, "", &text);

    let message = refusal(&pr(), &[by("tutros", body)]);

    assert!(
        message.contains("says it has 1 must-fix finding(s), but 2 could be read"),
        "{message}"
    );
}

#[test]
fn a_review_with_no_must_fix_count_is_refused() {
    let text = review(HEAD, 1).replace("Must-fix findings: 1\n", "");
    let body = pr_comment(7, "", &text);

    let message = refusal(&pr(), &[by("tutros", body)]);

    assert!(
        message.contains(&format!(
            "the latest sbxm review of PR #7 (commit {HEAD}) has no `Must-fix findings: <n>` \
             line, so whether all its must-fix findings were read can't be told"
        )),
        "{message}"
    );
}

/// A complete review whose count matches still gives every must-fix finding.
#[test]
fn a_long_review_that_was_not_cut_and_whose_count_matches_runs() {
    let body = pr_comment(7, "", &padded(3, 2, 50_000));
    assert!(!body.contains("sbxm cut this review"));

    let chosen = review_to_fix(&pr(), "o/r", None, "tutros", &[by("tutros", body)]).unwrap();

    assert_eq!(chosen.must_fix, 3);
    assert!(chosen.text.contains("### M-3"));
}
