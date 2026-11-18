//! The readiness rules (issue 725): what an issue needs before a worker can take
//! it, checked by `sbxm task ready-check`, `task loop --check` and the loop before a start. The
//! rules read the issue's labels and its body outside code fences; a rule can pass, fail, warn or
//! be skipped, and only a fail makes the issue unready. The rules that read GitHub beyond the
//! issue (paths on the default branch, PRs merged since the label) only ever warn.

use serde::Serialize;

use super::finish::next_fence;
use super::queue::{self, READY_LABEL};
use crate::github::{GitHubBackend, Issue};

/// Rule 4's reason for an `## Open points` heading, as the loop has said it since issue 511.
pub const OPEN_POINTS: &str = "open points not answered";

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

/// Every rule, in their order: [`body_rules`] with the paths rule before `**Files:**`, then
/// staleness. Reads GitHub for the last two; a read that fails is a warn, never a fail.
pub fn check(repo: &str, github: &dyn GitHubBackend, issue: &Issue) -> Vec<Verdict> {
    let mut verdicts = body_rules(issue);
    let at = verdicts.len() - 1;
    verdicts.insert(at, paths(repo, github, &issue.body));
    verdicts.push(staleness(repo, github, issue));
    verdicts
}

/// The warns of [`check`], in the rules' order: what `task loop --check` and the loop say of an
/// issue they take or pass over, holding nothing back (review M-2).
pub fn warnings(repo: &str, github: &dyn GitHubBackend, issue: &Issue) -> Vec<Verdict> {
    check(repo, github, issue)
        .into_iter()
        .filter(|v| v.status == Status::Warn)
        .collect()
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

/// Rule 2: a `**Depends on:**` line, `none` or `#N` numbers (each maybe `PR #N`) separated by
/// commas, then any text.
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
        // A dependency may be a PR (`PR #377`), which the loop waits on as on an issue.
        let rest_number = rest.strip_prefix("PR ").unwrap_or(rest);
        let digits_at = rest_number.strip_prefix('#')?;
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
        return Verdict::new(RULE, Status::Fail, OPEN_POINTS);
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

/// A bold field label at a line's start, `**Name:**`, by its name.
fn bold_label(line: &str) -> Option<&str> {
    let name = line.trim_start().strip_prefix("**")?;
    let (name, _) = name.split_once(":**")?;
    (!name.is_empty() && !name.contains('*')).then_some(name)
}

/// Rule 5: an `Acceptance criteria` heading with at least one `- [ ]` box before the next heading.
/// The `**Acceptance criteria:**` label `task file-findings` writes (the code-review skill's issue
/// template) counts as the heading, its boxes ending at the next heading or bold label.
fn acceptance(lines: &[&str]) -> Verdict {
    const RULE: &str = "acceptance";
    let is_acceptance = |text: &str| {
        text.trim_end_matches(':')
            .eq_ignore_ascii_case("acceptance criteria")
    };
    let Some(at) = lines.iter().position(|l| {
        heading(l).is_some_and(|(_, text)| is_acceptance(text))
            || bold_label(l).is_some_and(is_acceptance)
    }) else {
        return Verdict::new(RULE, Status::Fail, "no Acceptance criteria heading");
    };
    let boxes = lines[at + 1..]
        .iter()
        .take_while(|l| heading(l).is_none() && bold_label(l).is_none())
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
/// each maybe marked `(new)`. Advice only, so a malformed line is a warn.
fn files(lines: &[&str]) -> Verdict {
    const RULE: &str = "files";
    let Some(value) = field(lines, "**Files:**") else {
        return Verdict::new(RULE, Status::Skip, "no **Files:** line");
    };
    if files_well_formed(value) {
        let count = value.split(',').count();
        Verdict::new(RULE, Status::Pass, format!("{count} paths"))
    } else {
        Verdict::new(
            RULE,
            Status::Warn,
            format!("**Files:** `{value}` is not a comma-separated list of backticked paths"),
        )
    }
}

/// Whether `value`, a `**Files:**` line's text, is a comma-separated list of backticked paths,
/// each maybe marked `(new)`.
pub fn files_well_formed(value: &str) -> bool {
    value.split(',').map(str::trim).all(|entry| {
        let entry = entry.strip_suffix("(new)").map_or(entry, str::trim_end);
        entry.len() > 2
            && entry.starts_with('`')
            && entry.ends_with('`')
            && !entry[1..entry.len() - 1].contains('`')
    })
}

/// A repository path the body names in backticks, and whether it is marked `(new)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedPath {
    pub path: String,
    pub new: bool,
}

/// Every repository path `body` names in backticks outside code fences, once each, in order: a
/// word of path characters with a `/` or a file extension, not an absolute path, an option or a
/// parent folder. One followed by `(new)` is marked new.
pub fn named_paths(body: &str) -> Vec<NamedPath> {
    let mut named: Vec<NamedPath> = Vec::new();
    for line in unfenced(body) {
        let mut spans = line.split('`');
        // The text before the first backtick is outside any span.
        spans.next();
        while let (Some(inside), Some(after)) = (spans.next(), spans.next()) {
            let new = after.trim_start().starts_with("(new)");
            let Some(path) = repo_path(inside) else {
                continue;
            };
            match named.iter_mut().find(|n| n.path == path) {
                Some(seen) => seen.new |= new,
                None => named.push(NamedPath {
                    path: path.to_owned(),
                    new,
                }),
            }
        }
    }
    named
}

/// `text` as a repository path, `./` dropped, or `None` when it isn't one.
fn repo_path(text: &str) -> Option<&str> {
    let path = text.strip_prefix("./").unwrap_or(text);
    let chars_ok = !path.is_empty()
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'));
    if !chars_ok || path.starts_with(['/', '-']) || path.contains("//") {
        return None;
    }
    if path.split('/').any(|segment| segment == "..") {
        return None;
    }
    let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let extension = name
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .is_some_and(|(_, ext)| ext.chars().any(|c| c.is_ascii_alphabetic()));
    (path.contains('/') || extension).then_some(path)
}

/// Rule 6: every path the body names exists on the default branch, unless it is marked `(new)`,
/// from one listing. A missing path is a warn: issue text names paths loosely.
fn paths(repo: &str, github: &dyn GitHubBackend, body: &str) -> Verdict {
    const RULE: &str = "paths";
    let wanted: Vec<String> = named_paths(body)
        .into_iter()
        .filter(|n| !n.new)
        .map(|n| n.path)
        .collect();
    if wanted.is_empty() {
        return Verdict::new(RULE, Status::Skip, "the body names no existing path");
    }
    let listing = github
        .default_branch(repo)
        .and_then(|branch| github.repo_paths(repo, &branch));
    let listed = match listing {
        Ok(listed) => listed,
        Err(e) => {
            return Verdict::new(
                RULE,
                Status::Warn,
                format!("the default branch's paths could not be listed ({e:#})"),
            );
        }
    };
    let missing: Vec<&str> = wanted
        .iter()
        .map(String::as_str)
        .filter(|path| !listed.iter().any(|l| names(path, l)))
        .collect();
    if missing.is_empty() {
        Verdict::new(
            RULE,
            Status::Pass,
            format!("{} paths exist on the default branch", wanted.len()),
        )
    } else {
        Verdict::new(
            RULE,
            Status::Warn,
            format!(
                "not on the default branch (mark a path to add `(new)`): {}",
                missing.join(", ")
            ),
        )
    }
}

/// Whether `named`, a path the body names (a folder may end in `/`), is `path` or a folder it is
/// in.
fn names(named: &str, path: &str) -> bool {
    let named = named.trim_end_matches('/');
    path == named
        || path
            .strip_prefix(named)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Rule 8 (decision 295): the PRs merged since [`READY_LABEL`] was last added that changed a path
/// the body names, as a warn: the issue may no longer match the code.
fn staleness(repo: &str, github: &dyn GitHubBackend, issue: &Issue) -> Verdict {
    const RULE: &str = "staleness";
    if !issue.labels.iter().any(|l| l == READY_LABEL) {
        return Verdict::new(RULE, Status::Skip, format!("not labeled {READY_LABEL}"));
    }
    let named: Vec<String> = named_paths(&issue.body)
        .into_iter()
        .map(|n| n.path)
        .collect();
    if named.is_empty() {
        return Verdict::new(RULE, Status::Skip, "the body names no path");
    }
    match changed_since_label(repo, github, issue.number, &named) {
        Ok(None) => Verdict::new(
            RULE,
            Status::Skip,
            format!("no {READY_LABEL} labelling found in the issue's events"),
        ),
        Ok(Some((since, changed))) if changed.is_empty() => Verdict::new(
            RULE,
            Status::Pass,
            format!("no PR merged since it was labeled ({since}) changed a path it names"),
        ),
        Ok(Some((since, changed))) => Verdict::new(
            RULE,
            Status::Warn,
            format!(
                "merged since it was labeled ({since}) and changed paths it names, so check it \
                 still holds: {}",
                changed.join(", ")
            ),
        ),
        Err(e) => Verdict::new(
            RULE,
            Status::Warn,
            format!("the PRs merged since the label could not be read ({e:#})"),
        ),
    }
}

/// When issue `number` was last labeled [`READY_LABEL`] (its day) and each PR merged since that
/// changed a path of `named`, as `#N (path, path)`; `None` without such a labelling.
fn changed_since_label(
    repo: &str,
    github: &dyn GitHubBackend,
    number: u32,
    named: &[String],
) -> anyhow::Result<Option<(String, Vec<String>)>> {
    let Some(at) = github
        .label_events(repo, number)?
        .into_iter()
        .filter(|e| e.added && e.label == READY_LABEL)
        .map(|e| e.at)
        .max()
    else {
        return Ok(None);
    };
    let stamp = crate::run::results::format_timestamp(at);
    let day = &stamp[..10];
    let mut changed = Vec::new();
    let mut merged = github.prs_merged_since(repo, day)?;
    merged.retain(|pr| pr.merged_at >= at);
    merged.sort_by_key(|pr| pr.number);
    for pr in merged {
        let files = github.pr_files(repo, pr.number)?;
        let mut hits: Vec<&str> = Vec::new();
        for file in &files.files {
            let paths = std::iter::once(&file.filename).chain(&file.previous_filename);
            for path in paths {
                if named.iter().any(|n| names(n, path)) && !hits.contains(&path.as_str()) {
                    hits.push(path);
                }
            }
        }
        if !hits.is_empty() {
            changed.push(format!("#{} ({})", pr.number, hits.join(", ")));
        }
    }
    Ok(Some((day.to_owned(), changed)))
}
