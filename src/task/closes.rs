//! An issue body's `**Closes on merge:** no` line (issue 506): the issue's PR must leave it open.
//! `task finish` then writes `Refs #N` in the PR description instead of `Fixes #N`, and the
//! worker prompt asks for `Refs #N` in the commits. Read from the issue text the start saved
//! (`issue.md`, `gh issue view` output); a spec task never reads it.

use super::finish::next_fence;

/// The line's label, at the start of a line of the issue's body.
pub const LABEL: &str = "**Closes on merge:**";

/// Whether `issue` (`gh issue view` output: a header, a `--` line, then the body) asks for its PR
/// to leave it open: a body line that starts with [`LABEL`] and has the value `no` in any case,
/// outside a code fence. Any other value, or no such line, closes the issue as before. Text
/// without a `--` line is all body.
pub fn leaves_open(issue: &str) -> bool {
    let mut open = false;
    for line in body_lines(issue) {
        if let Some(value) = line.strip_prefix(LABEL) {
            open = value.trim().eq_ignore_ascii_case("no");
        }
    }
    open
}

/// The lines of `issue`'s body (after its `--` line; all of it without one) that are outside a
/// code fence, fence lines left out: where a `**Label:**` line of the body is read.
pub(crate) fn body_lines(issue: &str) -> Vec<&str> {
    let body = match issue.lines().position(|line| line.trim_end() == "--") {
        Some(at) => issue.lines().skip(at + 1).collect::<Vec<_>>(),
        None => issue.lines().collect(),
    };
    let mut fence = None;
    let mut lines = Vec::new();
    for line in body {
        let inside = fence.is_some();
        fence = next_fence(fence, line);
        if !(inside || fence.is_some()) {
            lines.push(line);
        }
    }
    lines
}
