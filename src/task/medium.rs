//! The reasons `task loop` doesn't arm auto-merge on a medium-risk PR besides its risk (issue
//! 549, #460 guardrail 6): its issue isn't `size:S` or `size:M`, read by [`queue::size`] from the
//! issue's own labels at arming time (issue 709; a queued spec job's PR has no issue and counts
//! as S), or its diff changes a `Cargo.lock`, read from the PR's file list. Anything that can't be
//! read is a reason too. Low risk never comes here (decisions 266, 280).

use crate::github::{GitHubBackend, PrFile};
use crate::task::queue::{self, SizeError};

/// Where a PR's task came from, for its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A GitHub issue: its size is its label's.
    Issue(u32),
    /// A queued spec job (issue 524): no issue, counts as size S.
    Spec,
    /// Not known (no readable task record, or a task of another kind).
    Unknown,
}

/// Every reason not to arm PR `number`, a medium-risk PR from `source`, in the order size, then
/// `Cargo.lock`; empty when both pass. Reads GitHub, never writes it.
pub fn refusals(
    github: &dyn GitHubBackend,
    repo: &str,
    source: Source,
    number: u32,
) -> Vec<String> {
    let mut reasons = Vec::new();
    if let Some(reason) = size_refusal(github, repo, source) {
        reasons.push(reason);
    }
    match github.pr_files(repo, number) {
        Ok(files) => {
            if files.files.len() != files.changed_files as usize {
                reasons.push(format!(
                    "its file list doesn't match its changed-files count ({} listed, {} changed)",
                    files.files.len(),
                    files.changed_files
                ));
            }
            if let Some(path) = files.files.iter().find_map(cargo_lock) {
                reasons.push(format!("it changes {path}"));
            }
        }
        Err(e) => reasons.push(format!(
            "its file list could not be read: {}",
            first_line(&format!("{e:#}"))
        )),
    }
    reasons
}

fn size_refusal(github: &dyn GitHubBackend, repo: &str, source: Source) -> Option<String> {
    let issue = match source {
        Source::Spec => return None,
        Source::Unknown => return Some("the issue it came from is unknown".to_owned()),
        Source::Issue(issue) => issue,
    };
    let labels = match github.issue_label_names(repo, issue) {
        Ok(labels) => labels,
        Err(e) => {
            return Some(format!(
                "its issue #{issue}'s labels could not be read: {}",
                first_line(&format!("{e:#}"))
            ));
        }
    };
    match queue::size(&labels) {
        Ok("S" | "M") => None,
        Ok(size) => Some(format!("its issue #{issue} is size {size}")),
        Err(SizeError::Missing) => Some(format!("its issue #{issue} has no size label")),
        Err(several) => Some(format!("its issue #{issue} has {several}")),
    }
}

/// The `Cargo.lock` path `file` adds, changes, deletes or renames to or from, in any directory.
fn cargo_lock(file: &PrFile) -> Option<&str> {
    let named = |path: &str| path.rsplit('/').next() == Some("Cargo.lock");
    std::iter::once(file.filename.as_str())
        .chain(file.previous_filename.as_deref())
        .find(|path| named(path))
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}
