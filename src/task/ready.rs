//! The readiness rules (issue 725, decision 331(1)): what an issue needs before a worker can take
//! it, checked by `sbxm task ready-check`, `task loop --check` and the loop before a start. The
//! rules read the issue's labels and its body outside code fences; a rule can pass, fail, warn or
//! be skipped, and only a fail makes the issue unready. The rules that read GitHub beyond the
//! issue (paths on the default branch, PRs merged since the label) only ever warn.

use serde::Serialize;

use super::finish::next_fence;
use super::queue;
use crate::github::Issue;

/// What a rule found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Fail,
    Warn,
    /// The rule has nothing to check (an optional line that isn't there).
    Skip,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Warn => "warn",
            Self::Skip => "skip",
        }
    }
}

/// One rule's verdict, with its reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verdict {
    pub rule: &'static str,
    pub status: Status,
    pub reason: String,
}

impl Verdict {
    fn new(rule: &'static str, status: Status, reason: impl Into<String>) -> Self {
        Self {
            rule,
            status,
            reason: reason.into(),
        }
    }
}

/// The rules that read the issue alone, in their order: one size, `**Depends on:**`, `**Real
/// check:**`, nothing left open, acceptance criteria, then `**Files:**` (advice, so only a warn).
pub fn body_rules(issue: &Issue) -> Vec<Verdict> {
    let lines = unfenced(&issue.body);
    vec![
        size(issue, &lines),
        depends_on(&lines),
        real_check(&lines),
        open_points(issue, &lines),
        acceptance(&lines),
        files(&lines),
    ]
}

/// The reason of every rule `issue` fails, in the rules' order: what the loop and `task loop
/// --check` skip it for. Warns never count.
pub fn failures(issue: &Issue) -> Vec<String> {
    body_rules(issue)
        .into_iter()
        .filter(|v| v.status == Status::Fail)
        .map(|v| v.reason)
        .collect()
}

/// The lines of `body` outside code fences, fence lines left out.
fn unfenced(body: &str) -> Vec<&str> {
    let mut fence = None;
    let mut lines = Vec::new();
    for line in body.lines() {
        let inside = fence.is_some();
        fence = next_fence(fence, line);
        if !(inside || fence.is_some()) {
            lines.push(line);
        }
    }
    lines
}

/// The value of the first line that starts with `label`, trimmed.
fn field<'a>(lines: &[&'a str], label: &str) -> Option<&'a str> {
    lines
        .iter()
        .find_map(|line| line.trim_start().strip_prefix(label))
        .map(str::trim)
}

/// Rule 1: exactly one `size:*` label (the one size rule of issue 709) and a `**Size:**` line
/// naming the same size.
fn size(issue: &Issue, lines: &[&str]) -> Verdict {
    const RULE: &str = "size";
    let label = match queue::size(&issue.labels) {
        Ok(size) => size,
        Err(e) => return Verdict::new(RULE, Status::Fail, e.to_string()),
    };
    let Some(value) = field(lines, "**Size:**") else {
        return Verdict::new(
            RULE,
            Status::Fail,
            format!("labeled size:{label} but the body has no **Size:** line"),
        );
    };
    match line_size(value) {
        Some(line) if line == label => Verdict::new(
            RULE,
            Status::Pass,
            format!("size:{label}, as its **Size:** line"),
        ),
        Some(line) => Verdict::new(
            RULE,
            Status::Fail,
            format!("labeled size:{label} but its **Size:** line says {line}"),
        ),
        None => Verdict::new(
            RULE,
            Status::Fail,
            format!("labeled size:{label} but its **Size:** line names no size (S, M or L)"),
        ),
    }
}

/// The size a `**Size:**` line's value starts with: `M: why`, `` `size:M` ``, `L (why)`.
fn line_size(value: &str) -> Option<&'static str> {
    let value = value.trim_start_matches(['`', '*', ' ']);
    let value = value.strip_prefix("size:").unwrap_or(value);
    let mut chars = value.chars();
    let size = match chars.next()? {
        'S' => "S",
        'M' => "M",
        'L' => "L",
        _ => return None,
    };
    match chars.next() {
        Some(c) if c.is_alphanumeric() => None,
        _ => Some(size),
    }
}

/// Rule 2: a `**Depends on:**` line, `none` or `#N` numbers separated by commas, then any text.
fn depends_on(lines: &[&str]) -> Verdict {
    const RULE: &str = "depends-on";
    let Some(value) = field(lines, "**Depends on:**") else {
        return Verdict::new(
            RULE,
            Status::Fail,
            "no **Depends on:** line (`none`, or the issues it waits for as #N)",
        );
    };
    match dependencies(value) {
        Some(numbers) if numbers.is_empty() => Verdict::new(RULE, Status::Pass, "none"),
        Some(numbers) => {
            let list: Vec<String> = numbers.iter().map(|n| format!("#{n}")).collect();
            Verdict::new(RULE, Status::Pass, list.join(", "))
        }
        None => Verdict::new(
            RULE,
            Status::Fail,
            format!(
                "**Depends on:** `{value}` is neither `none` nor #N numbers separated by commas"
            ),
        ),
    }
}

/// The numbers of a `**Depends on:**` value, none for `none`; `None` when it is malformed.
fn dependencies(value: &str) -> Option<Vec<u32>> {
    if value
        .get(..4)
        .is_some_and(|v| v.eq_ignore_ascii_case("none"))
    {
        return value[4..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric())
            .then(Vec::new);
    }
    let mut numbers = Vec::new();
    let mut rest = value;
    loop {
        let digits_at = rest.strip_prefix('#')?;
        let digits: String = digits_at.chars().take_while(char::is_ascii_digit).collect();
        numbers.push(digits.parse().ok()?);
        rest = &digits_at[digits.len()..];
        // Another number follows a comma; anything else is the line's own text.
        match rest.trim_start().strip_prefix(',') {
            Some(next) => rest = next.trim_start(),
            None => return Some(numbers),
        }
    }
}

/// Rule 3: a `**Real check:**` line, if there is one, names a check, on it or on the next
/// non-empty line (as [`super::real_check::required`] reads it).
fn real_check(lines: &[&str]) -> Verdict {
    const RULE: &str = "real-check";
    let label = super::real_check::LABEL;
    let Some(at) = lines.iter().position(|l| l.starts_with(label)) else {
        return Verdict::new(RULE, Status::Skip, "no **Real check:** line");
    };
    let value = lines[at][label.len()..].trim();
    let check = if value.is_empty() {
        lines[at + 1..]
            .iter()
            .map(|line| line.trim())
            .find(|line| !line.is_empty())
            // The next line is the check unless it is another field or a heading.
            .filter(|line| !line.starts_with("**") && !line.starts_with('#'))
            .map(|line| line.trim_start_matches(['-', '*', '+']).trim())
            .filter(|line| !line.is_empty())
    } else {
        Some(value)
    };
    match check {
        Some(check) => Verdict::new(RULE, Status::Pass, check.to_owned()),
        None => Verdict::new(RULE, Status::Fail, "the **Real check:** line is empty"),
    }
}

/// Rule 4: no `**Open:**` line, no `## Open points` heading and no "to decide", in any case.
fn open_points(issue: &Issue, lines: &[&str]) -> Verdict {
    const RULE: &str = "open-points";
    if lines
        .iter()
        .any(|l| l.trim_start().starts_with("**Open:**"))
    {
        return Verdict::new(RULE, Status::Fail, "an **Open:** line is still there");
    }
    if queue::has_open_points(&issue.body) {
        return Verdict::new(RULE, Status::Fail, "an Open points heading is still there");
    }
    if lines.iter().any(|l| l.to_lowercase().contains("to decide")) {
        return Verdict::new(RULE, Status::Fail, "the body still says \"to decide\"");
    }
    Verdict::new(RULE, Status::Pass, "nothing left open")
}

/// A Markdown heading's level and text.
fn heading(line: &str) -> Option<(usize, &str)> {
    let hashes = line.len() - line.trim_start_matches('#').len();
    let text = line[hashes..].strip_prefix(' ')?;
    (1..=6).contains(&hashes).then(|| (hashes, text.trim()))
}

/// Rule 5: an `Acceptance criteria` heading with at least one `- [ ]` box before the next heading.
fn acceptance(lines: &[&str]) -> Verdict {
    const RULE: &str = "acceptance";
    let Some(at) = lines.iter().position(|l| {
        heading(l).is_some_and(|(_, text)| {
            text.trim_end_matches(':')
                .eq_ignore_ascii_case("acceptance criteria")
        })
    }) else {
        return Verdict::new(RULE, Status::Fail, "no Acceptance criteria heading");
    };
    let boxes = lines[at + 1..]
        .iter()
        .take_while(|l| heading(l).is_none())
        .filter(|l| l.trim_start().starts_with("- [ ]"))
        .count();
    if boxes == 0 {
        Verdict::new(
            RULE,
            Status::Fail,
            "the Acceptance criteria heading has no `- [ ]` box",
        )
    } else {
        Verdict::new(RULE, Status::Pass, format!("{boxes} open boxes"))
    }
}

/// Rule 7: a `**Files:**` line, if there is one, is a comma-separated list of backticked paths,
/// each maybe marked `(new)`. Advice only (decision 331(2)), so a malformed line is a warn.
fn files(lines: &[&str]) -> Verdict {
    const RULE: &str = "files";
    let Some(value) = field(lines, "**Files:**") else {
        return Verdict::new(RULE, Status::Skip, "no **Files:** line");
    };
    let entries: Vec<&str> = value.split(',').map(str::trim).collect();
    let well_formed = entries.iter().all(|entry| {
        let entry = entry.strip_suffix("(new)").map_or(*entry, str::trim_end);
        entry.len() > 2
            && entry.starts_with('`')
            && entry.ends_with('`')
            && !entry[1..entry.len() - 1].contains('`')
    });
    if well_formed {
        Verdict::new(RULE, Status::Pass, format!("{} paths", entries.len()))
    } else {
        Verdict::new(
            RULE,
            Status::Warn,
            format!("**Files:** `{value}` is not a comma-separated list of backticked paths"),
        )
    }
}
