//! `sbxm task fix --pr N` (issue 313): which review of a pull request its worker fixes. The
//! latest comment by the authenticated `gh` user that carries sbxm's review header counts; any
//! other comment is never read into a prompt. Its `Scope:` range must end at the PR's head, and
//! only its must-fix findings are the worker's input.

use anyhow::{Result, bail};

use super::findings;
use super::review::{CUT_NOTE_START, review_pr_command};
use crate::github::{PrComment, PrInfo, PrState};

/// The review a fix task works from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewToFix {
    /// The PR's head, which the review's `Scope:` range ends at (full id).
    pub commit: String,
    /// How many must-fix findings the worker gets.
    pub must_fix: usize,
    /// The worker's input: the PR named, then each must-fix finding as the review wrote it.
    pub text: String,
}

/// The first line of a comment `task review --pr` posts, and the start of the one `task finish`
/// and `task fix` post (see [`super::review::pr_comment`], [`super::review::in_task_pr_comment`]).
fn has_review_header(body: &str, number: u32) -> bool {
    let own = format!("sbxm review of PR #{number}");
    findings::lines_of(body)
        .iter()
        .any(|line| *line == own || line.starts_with("sbxm in-task review of "))
}

/// The `Must-fix findings: <n>` count anywhere in the comment (it follows the header there).
fn stated_must_fix(body: &str) -> Option<u32> {
    findings::lines_of(body).iter().find_map(|line| {
        line.trim_end()
            .strip_prefix("Must-fix findings: ")
            .and_then(|n| n.parse().ok())
    })
}

/// Only an open PR from a branch of this repo has a branch sbxm may push to.
pub fn check_fixable(pr: &PrInfo, repo: &str, base: Option<&str>) -> Result<()> {
    let number = pr.number;
    match pr.state {
        PrState::Open => {}
        PrState::Closed => bail!("PR #{number} is closed; there is no open branch to push to"),
        PrState::Merged => bail!("PR #{number} is merged; there is no open branch to push to"),
    }
    if pr.is_cross_repository {
        let fork = if pr.head_repo.is_empty() {
            "another repository"
        } else {
            pr.head_repo.as_str()
        };
        bail!(
            "PR #{number} comes from a fork ({fork}); sbxm can push only to this repo's own \
             branches; `{}` still works",
            review_pr_command(number, repo, base, false)
        );
    }
    Ok(())
}

/// The review of `pr` to fix, from its `comments` (oldest first) and the authenticated `user`:
/// refused, with the reason and the command that helps, for a PR sbxm can't push to, no review by
/// `user`, a review of an older head, or one with nothing to fix. `base` only goes into the
/// `task review --pr` command a refusal names.
pub fn review_to_fix(
    pr: &PrInfo,
    repo: &str,
    base: Option<&str>,
    user: &str,
    comments: &[PrComment],
) -> Result<ReviewToFix> {
    let number = pr.number;
    check_fixable(pr, repo, base)?;
    let review_again = review_pr_command(number, repo, base, false);
    // The local `pr-N` task records the head an unusable review covers (or may cover) as
    // reviewed, so only a forced review runs again; a review of an older head needs no force.
    let review_head_again = review_pr_command(number, repo, base, true);
    let Some(latest) = comments
        .iter()
        .rev()
        .find(|c| c.author == user && has_review_header(&c.body, number))
    else {
        bail!("PR #{number} has no sbxm review by {user}; run `{review_again}` first");
    };
    let parsed = findings::parse(&latest.body);
    let Some(covered) = parsed.head_sha.as_deref() else {
        bail!(
            "the latest sbxm review of PR #{number} has no `Scope:` range, so which commit it \
             covers is unknown; review the head again with `{review_head_again}`"
        );
    };
    let covered = covered.to_ascii_lowercase();
    if !pr.head_oid.to_ascii_lowercase().starts_with(&covered) {
        bail!(
            "the latest sbxm review covers {covered}, but PR #{number}'s head is now {}; review \
             the new head first with `{review_again}`",
            pr.head_oid
        );
    }
    let commit = pr.head_oid.clone();
    let lines = findings::lines_of(&latest.body);
    // The worker's input must be every must-fix finding (decision 2): a review cut to fit
    // GitHub, or one whose stated count isn't what was read, is refused, never used in part.
    if lines.iter().any(|line| line.starts_with(CUT_NOTE_START)) {
        bail!(
            "the latest sbxm review of PR #{number} (commit {commit}) was cut to fit a GitHub \
             comment, so not all of its must-fix findings are on the PR; review the head again \
             with `{review_head_again}`"
        );
    }
    let must_fix: Vec<String> = parsed
        .findings
        .iter()
        .filter(|f| f.label == "must-fix")
        .map(|f| lines[f.start_line - 1..f.end_line.min(lines.len())].join("\n"))
        .collect();
    let Some(stated) = stated_must_fix(&latest.body) else {
        bail!(
            "the latest sbxm review of PR #{number} (commit {commit}) has no `Must-fix findings: \
             <n>` line, so whether all its must-fix findings were read can't be told; review the \
             head again with `{review_head_again}`"
        );
    };
    if stated == 0 && must_fix.is_empty() {
        bail!(
            "the latest sbxm review of PR #{number} (commit {commit}) has no must-fix findings, \
             so there is nothing to fix; file its other findings with `sbxm task file-findings`"
        );
    }
    if usize::try_from(stated).ok() != Some(must_fix.len()) {
        let read = match must_fix.len() {
            0 => "none".to_owned(),
            n => n.to_string(),
        };
        bail!(
            "the latest sbxm review of PR #{number} (commit {commit}) says it has {stated} \
             must-fix finding(s), but {read} could be read (each needs a `### M-1 - <title>` \
             heading under `## Must fix`); review the head again with `{review_head_again}`"
        );
    }
    let mut text = format!(
        "# The must-fix findings of the sbxm review of PR #{number}\n\n\
         Pull request #{number}: {}\nBranch: {}\nReviewed commit: {commit}\n",
        pr.title, pr.head_ref
    );
    for finding in &must_fix {
        text.push('\n');
        text.push_str(finding.trim_end());
        text.push('\n');
    }
    Ok(ReviewToFix {
        commit,
        must_fix: must_fix.len(),
        text,
    })
}
