//! The real check an issue's PR needs before `task loop` arms it (issue 547, #460 guardrail 3,
//! decision 273): an issue body line that starts with `**Real check:**`, read the way
//! [`super::closes`] reads `**Closes on merge:** no`. Its result is the commit status
//! [`CONTEXT`] on the PR's head, posted by `sbxm task real-check`.

use super::closes::body_lines;

/// The line's label, at the start of a line of the issue's body.
pub const LABEL: &str = "**Real check:**";

/// The commit status context `sbxm task real-check` posts and the loop's arming reads.
pub const CONTEXT: &str = "sbxm/real-check";

/// GitHub's limit on a commit status's description, in characters.
pub const NOTE_LIMIT: usize = 140;

/// The real check `issue` (`gh issue view` output: a header, a `--` line, then the body) names,
/// by its first line: the value of the last body line outside a code fence that starts with
/// [`LABEL`], or for an empty value the next non-empty line, list marks dropped. `None` without
/// such a line, or for the value `none` in any case.
pub fn required(issue: &str) -> Option<String> {
    let lines = body_lines(issue);
    let mut check = None;
    for (at, line) in lines.iter().enumerate() {
        let Some(value) = line.strip_prefix(LABEL) else {
            continue;
        };
        let value = value.trim();
        check = if value.eq_ignore_ascii_case("none") {
            None
        } else if value.is_empty() {
            let next = lines[at + 1..]
                .iter()
                .map(|line| line.trim().trim_start_matches(['-', '*', '+']).trim())
                .find(|line| !line.is_empty())
                .unwrap_or("(see the issue)");
            Some(next.to_owned())
        } else {
            Some(value.to_owned())
        };
    }
    check
}
