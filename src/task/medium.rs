//! The reasons `task loop` doesn't arm auto-merge on a medium-risk PR besides its risk (issue
//! 549, #460 guardrail 6): its issue isn't `size:S` or `size:M`, read from the issue's labels at
//! arming time (a queued spec job's PR has no issue and counts as S), or its diff changes a
//! `Cargo.lock`, read from the PR's file list. Anything that can't be read is a reason too. Low
//! risk never comes here (decisions 266, 280).

use crate::github::{GitHubBackend, PrFile};
use crate::task::queue;

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
    let text = match github.issue(repo, issue) {
        Ok(text) => text.text,
        Err(e) => {
            return Some(format!(
                "its issue #{issue}'s labels could not be read: {}",
                first_line(&format!("{e:#}"))
            ));
        }
    };
    let sizes: Vec<&str> = labels(&text)
        .iter()
        .filter_map(|label| queue::size(std::slice::from_ref(label)))
        .collect();
    match sizes.as_slice() {
        [] => Some(format!("its issue #{issue} has no size label")),
        ["S" | "M"] => None,
        [size] => Some(format!("its issue #{issue} is size {size}")),
        _ => Some(format!(
            "its issue #{issue} has more than one size label ({})",
            sizes.join(", ")
        )),
    }
}

/// The labels on `gh issue view`'s `labels:` header line, comma separated; none without one.
fn labels(text: &str) -> Vec<String> {
    text.lines()
        .take_while(|line| *line != "--")
        .find_map(|line| line.strip_prefix("labels:"))
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
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
