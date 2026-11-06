//! A spec's `Fixes:` line (issue 438, decision 301): the issues a spec task's PR closes under
//! `[finish] sink = "pr"`. One plain line, `Fixes: #435, #436`, anywhere in the spec; numbers
//! the spec only mentions are never used. Read at `task start --spec` from the spec file, and at
//! `task finish --spec` from the copy the start saved, so the PR fixes what the start checked.

use anyhow::{Result, bail};

use crate::github::GitHubBackend;

/// The form a `Fixes:` line takes, as the refusals show it.
pub const FORM: &str = "Fixes: #435, #436";

/// The numbers of `text`'s `Fixes:` line, in the line's order and each once; empty when it has
/// none. A line that looks meant as one (it starts with the word "Fixes" and then `:`, `#` or a
/// digit, in any case) but isn't exactly the form is refused, naming the line, and so are two of
/// them, as ambiguous.
pub fn parse(text: &str) -> Result<Vec<u32>> {
    let mut found: Option<(usize, Vec<u32>)> = None;
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim_end();
        let at = index + 1;
        if !looks_like_fixes(line) {
            continue;
        }
        let Some(numbers) = numbers_of(line) else {
            bail!(
                "the spec's line {at}, `{}`, isn't a Fixes line sbxm can read; write it as `{FORM}` \
                 (the line starts with `Fixes:`, then each issue as #<number>, separated by commas)",
                line.trim()
            );
        };
        if let Some((first, _)) = &found {
            bail!(
                "the spec has two Fixes lines (lines {first} and {at}), which is ambiguous; keep \
                 one, `{FORM}`, listing every issue the PR closes"
            );
        }
        found = Some((at, numbers));
    }
    Ok(found.map(|(_, numbers)| numbers).unwrap_or_default())
}

/// Whether `line` starts with the word "fixes" (any case, after any indentation) followed by `:`,
/// `#` or a digit: meant as a `Fixes:` line, well formed or not.
fn looks_like_fixes(line: &str) -> bool {
    let line = line.trim_start();
    let Some(word) = line.get(..5) else {
        return false;
    };
    if !word.eq_ignore_ascii_case("fixes") {
        return false;
    }
    line[5..]
        .trim_start()
        .starts_with(|c: char| c == ':' || c == '#' || c.is_ascii_digit())
}

/// The numbers of a well-formed line, duplicates dropped; `None` when it isn't the form.
fn numbers_of(line: &str) -> Option<Vec<u32>> {
    let rest = line.strip_prefix("Fixes:")?;
    let mut numbers = Vec::new();
    for item in rest.split(',') {
        let digits = item.trim().strip_prefix('#')?;
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let number: u32 = digits.parse().ok().filter(|n| *n > 0)?;
        if !numbers.contains(&number) {
            numbers.push(number);
        }
    }
    Some(numbers)
}

/// The numbers of `text`'s `Fixes:` line ([`parse`]), each checked to be an open issue of `repo`
/// ([`check_open_issues`]): the start's check of a spec under the `pr` sink, also made for each
/// spec target of a `task run` list before the first one starts (decision 193).
pub fn check_spec(github: &dyn GitHubBackend, repo: &str, text: &str) -> Result<Vec<u32>> {
    let numbers = parse(text)?;
    check_open_issues(github, repo, &numbers)?;
    Ok(numbers)
}

/// Refuses, naming it, the first of `numbers` that isn't an open issue of `repo`: one `gh` can't
/// read as an issue (it doesn't exist), a closed one, or a pull request. Checked at the start,
/// before any sandbox, so the PR's `Fixes` lines never close something else.
pub fn check_open_issues(github: &dyn GitHubBackend, repo: &str, numbers: &[u32]) -> Result<()> {
    for &number in numbers {
        let issue = match github.issue(repo, number) {
            Ok(issue) => issue,
            Err(e) => bail!(
                "the spec's Fixes line names #{number}, which isn't an issue of {repo} gh can \
                 read ({e:#}); list only open issues of {repo} there"
            ),
        };
        if issue.state != "OPEN" {
            bail!(
                "the spec's Fixes line names #{number}, which is {} on {repo}, not open; remove \
                 it from the line",
                issue.state.to_lowercase()
            );
        }
        // `gh issue view` may show a pull request too; `gh pr view` only knows pull requests.
        if github.pr(repo, number).is_ok() {
            bail!(
                "the spec's Fixes line names #{number}, which is a pull request of {repo}, not an \
                 issue; list only issues there"
            );
        }
    }
    Ok(())
}

/// The warning a start gives for a spec with no `Fixes:` line under the `pr` sink.
pub const NO_LINE_WARNING: &str = "the spec has no `Fixes:` line, so its PR closes no issue; add \
     one (`Fixes: #435, #436`) if it should";

/// The PR title for a spec: its first `# ` heading, else `fallback` (the spec file's name).
pub fn title(text: &str, fallback: &str) -> String {
    text.lines()
        .find_map(|line| line.strip_prefix("# "))
        .map(str::trim)
        .filter(|heading| !heading.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}
