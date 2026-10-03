//! The findings of a review file (spec §11, decisions 134, 149): parse `sdlc/reviews/*.md` or a
//! task's `review.md`, and later render, check and file them. Ported from
//! the PowerShell script removed in decision 165; its cases are the golden tests in `tests/task_findings_*.rs`.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid pattern")
}

/// The fields every finding needs (canonical, lower-case names).
const REQUIRED: [&str; 4] = ["where", "what happens", "why it matters", "fix"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub id: String,
    pub title: String,
    /// `must-fix`, `should-fix` or `question`.
    pub label: String,
    /// 1-based line of the heading, and of the last line of the body.
    pub start_line: usize,
    pub end_line: usize,
    /// Non-empty fields in file order, under canonical lower-case names.
    pub fields: Vec<(String, String)>,
    /// The `- [ ]` items under `Acceptance criteria`.
    pub criteria: Vec<String>,
    /// Required fields that are absent.
    pub missing: Vec<&'static str>,
}

impl Finding {
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Review {
    pub findings: Vec<Finding>,
    /// Sections with no finding (Summary, Verification), and nits in them: reported, never filed.
    pub not_filed_sections: usize,
    pub not_filed_nits: usize,
    /// The full sha after `..` in the Scope line, and the whole `a..b` range.
    pub head_sha: Option<String>,
    pub range: Option<String>,
    pub scope: Option<String>,
    pub scope_line: Option<usize>,
    /// The first `Issues:` line and how many there are.
    pub issues_line: Option<String>,
    pub issues_line_count: usize,
}

/// A review's lines, with CRLF read as LF.
pub fn lines_of(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect()
}

fn section_label(name: &str) -> Option<&'static str> {
    match name {
        "must fix" => Some("must-fix"),
        "should fix" => Some("should-fix"),
        "question" | "questions" => Some("question"),
        _ => None,
    }
}

fn canonical_field(name: &str) -> String {
    let name = name.trim().to_lowercase();
    match name.as_str() {
        "smallest fix" | "recommendation" | "options and recommendation" => "fix".to_owned(),
        _ => name,
    }
}

struct Section {
    label: Option<&'static str>,
    findings: usize,
    nits: usize,
}

struct Open {
    id: String,
    title: String,
    label: &'static str,
    start_line: usize,
    body: Vec<String>,
}

/// Text to a review: Scope, Issues line, findings and what isn't filed.
pub fn parse(text: &str) -> Review {
    static SCOPE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^Scope:\s*(.*)$"));
    static SHA: LazyLock<Regex> =
        LazyLock::new(|| re(r"(?i)\b[0-9a-f]{7,40}\.\.([0-9a-f]{7,40})\b"));
    static ISSUES: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^Issues:"));
    static SECTION: LazyLock<Regex> = LazyLock::new(|| re(r"^##\s+(.+?)\s*$"));
    static HEADING: LazyLock<Regex> = LazyLock::new(|| {
        re(r"^###\s+(\S+?)(?:\s+(?:\u{2014}|\u{2013}|-)\s+|:\s+|\s+:\s+)(.+?)\s*$")
    });
    static NIT: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^\s*[-*]\s+Nit:"));

    let mut review = Review::default();
    let mut sections: Vec<Section> = Vec::new();
    let mut open: Option<Open> = None;
    for (i, line) in lines_of(text).into_iter().enumerate() {
        let scope_taken = review.scope.as_deref().is_some_and(|s| !s.is_empty());
        if let (Some(c), false) = (SCOPE.captures(line), scope_taken) {
            let scope = c[1].to_owned();
            review.scope_line = Some(i + 1);
            if let Some(sha) = SHA.captures(&scope) {
                review.head_sha = Some(sha[1].to_owned());
                review.range = Some(sha[0].to_owned());
            }
            review.scope = Some(scope);
        } else if ISSUES.is_match(line) {
            review.issues_line_count += 1;
            review.issues_line.get_or_insert_with(|| line.to_owned());
        }
        if let Some(c) = SECTION.captures(line) {
            if let Some(done) = open.take() {
                review.findings.push(complete(done));
            }
            let name = c[1].trim_end_matches(':').trim().to_lowercase();
            sections.push(Section {
                label: section_label(&name),
                findings: 0,
                nits: 0,
            });
            continue;
        }
        if let (Some(section), Some(c)) = (sections.last_mut(), HEADING.captures(line))
            && let Some(label) = section.label
        {
            if let Some(done) = open.take() {
                review.findings.push(complete(done));
            }
            section.findings += 1;
            open = Some(Open {
                id: c[1].to_owned(),
                title: c[2].to_owned(),
                label,
                start_line: i + 1,
                body: Vec::new(),
            });
            continue;
        }
        if let Some(finding) = open.as_mut() {
            finding.body.push(line.to_owned());
        } else if let Some(section) = sections.last_mut()
            && section.label.is_none()
            && NIT.is_match(line)
        {
            section.nits += 1;
        }
    }
    if let Some(done) = open.take() {
        review.findings.push(complete(done));
    }
    for section in &sections {
        if section.findings == 0 {
            review.not_filed_sections += 1;
        }
        review.not_filed_nits += section.nits;
    }
    review
}

/// Splits a finding's body into fields and works out which required ones are missing.
fn complete(open: Open) -> Finding {
    static FIELD: LazyLock<Regex> = LazyLock::new(|| re(r"^\*\*([^*:]+):\*\*\s*(.*)$"));
    static ITEM: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*[-*]\s+\[[ xX]\]\s*(.+?)\s*$"));

    // In file order; a repeated field name starts that field again.
    let mut values: Vec<(String, Vec<&str>)> = Vec::new();
    for line in &open.body {
        if let Some(c) = FIELD.captures(line) {
            let name = canonical_field(&c[1]);
            let first = c.get(2).map_or("", |m| m.as_str());
            let lines = if first.is_empty() {
                Vec::new()
            } else {
                vec![first]
            };
            match values.iter_mut().find(|(n, _)| *n == name) {
                Some(slot) => slot.1 = lines,
                None => values.push((name, lines)),
            }
        } else if let Some((_, lines)) = values.last_mut() {
            lines.push(line);
        }
    }
    let mut fields = Vec::new();
    let mut criteria = Vec::new();
    for (name, lines) in values {
        if name == "acceptance criteria" {
            criteria.extend(
                lines
                    .iter()
                    .filter_map(|l| ITEM.captures(l).map(|c| c[1].to_owned())),
            );
        } else {
            let text = lines.join("\n").trim().to_owned();
            if !text.is_empty() {
                fields.push((name, text));
            }
        }
    }
    let missing = REQUIRED
        .into_iter()
        .filter(|r| !fields.iter().any(|(n, _)| n == r))
        .collect();
    Finding {
        end_line: open.start_line + open.body.len(),
        id: open.id,
        title: open.title,
        label: open.label.to_owned(),
        start_line: open.start_line,
        fields,
        criteria,
        missing,
    }
}

/// Secret patterns in the order they are reported. Like the script's `-match`, case-insensitive.
const SECRET_PATTERNS: [(&str, &str); 5] = [
    (
        "a GitHub token",
        r"(?i)\b(?:gh[pos]_[A-Za-z0-9]{16,}|github_pat_[A-Za-z0-9_]{16,})",
    ),
    ("an sk- key", r"(?i)\bsk-[A-Za-z0-9_-]{20,}"),
    ("an AWS key", r"(?i)\bAKIA[0-9A-Z]{16}\b"),
    ("a bearer token", r"(?i)Bearer\s+[A-Za-z0-9._~+/=-]{20,}"),
    ("a private key", r"(?i)-----BEGIN [A-Z ]*PRIVATE KEY-----"),
];

const ASSIGNMENT_KIND: &str = "a password or token assignment";

/// `password = x`, `api_token = "abc"`: a name, `=` (not `==`) and a non-empty value.
fn has_secret_assignment(text: &str) -> bool {
    static NAME: LazyLock<Regex> =
        LazyLock::new(|| re(r"(?i)(?:^|[^A-Za-z])(?:password|passwd|token|secret)\s*="));
    static VALUE: LazyLock<Regex> = LazyLock::new(|| re(r#"^\s*(?:"[^"]+"|'[^']+'|[^\s"']+)"#));
    NAME.find_iter(text).any(|m| {
        let rest = &text[m.end()..];
        !rest.starts_with('=') && VALUE.is_match(rest)
    })
}

/// The kind of secret a piece of text looks like, or nothing. Used per line and for text that
/// isn't tied to a finding's lines, such as the Scope line's fallback text or the file name.
pub fn secret_kind(text: &str) -> Option<&'static str> {
    static COMPILED: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
        SECRET_PATTERNS
            .iter()
            .map(|(kind, pattern)| (*kind, re(pattern)))
            .collect()
    });
    COMPILED
        .iter()
        .find(|(_, regex)| regex.is_match(text))
        .map(|(kind, _)| *kind)
        .or_else(|| has_secret_assignment(text).then_some(ASSIGNMENT_KIND))
}

/// A line of a finding that looks like a secret: never the value itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretHit {
    pub id: String,
    /// 1-based line in the review file.
    pub line: usize,
    pub kind: &'static str,
}

/// Lines of a finding (heading to last body line) that look like a secret.
pub fn find_secrets(text: &str, finding: &Finding) -> Vec<SecretHit> {
    let lines = lines_of(text);
    let last = finding.end_line.min(lines.len());
    (finding.start_line..=last)
        .filter_map(|n| {
            secret_kind(lines[n - 1]).map(|kind| SecretHit {
                id: finding.id.clone(),
                line: n,
                kind,
            })
        })
        .collect()
}

/// Personal paths become `~` (unless `keep_paths`) and e-mail addresses are flagged, each with a
/// warning. The profile folder may hold spaces ("Mary Jane Watson Parker"): when a separator,
/// quote or backtick ends it, the whole segment goes, however many words it has; otherwise only
/// the first word does, so prose after a bare path is left alone. A newline always ends it.
pub fn protect_text(text: &str, id: &str, keep_paths: bool, warnings: &mut Vec<String>) -> String {
    static PATHS: LazyLock<[Regex; 2]> = LazyLock::new(|| {
        [
            re(r#"(?i)\b[A-Z]:\\Users\\(?:([^\\\s`'"]+(?: [^\\\s`'"]+)*)([\\`'"])|[^\\\s`'"]+)"#),
            re(r#"/(?:home|Users)/(?:([^/\s`'"]+(?: [^/\s`'"]+)*)([/`'"])|[^/\s`'"]+)"#),
        ]
    });
    static EMAIL: LazyLock<Regex> = LazyLock::new(|| re(r"[\w.+-]+@[\w-]+(\.[\w-]+)+"));
    let mut text = text.to_owned();
    if !keep_paths {
        for pattern in PATHS.iter() {
            let hits = pattern.find_iter(&text).count();
            if hits > 0 {
                warnings.push(format!(
                    "{id}: replaced {hits} personal path(s) with ~; use --keep-paths to keep them"
                ));
                // The terminator is part of the match (no look-ahead in `regex`): put it back.
                text = pattern
                    .replace_all(&text, |c: &regex::Captures| match c.get(2) {
                        Some(end) => format!("~{}", end.as_str()),
                        None => "~".to_owned(),
                    })
                    .into_owned();
            }
        }
    }
    if EMAIL.is_match(&text) {
        warnings.push(format!("{id}: contains an e-mail address (left as is)"));
    }
    text
}

/// A copy of the finding with every text protected.
pub fn protect_finding(finding: &Finding, keep_paths: bool, warnings: &mut Vec<String>) -> Finding {
    let mut copy = finding.clone();
    copy.title = protect_text(&finding.title, &finding.id, keep_paths, warnings);
    for (_, value) in &mut copy.fields {
        *value = protect_text(value, &finding.id, keep_paths, warnings);
    }
    for item in &mut copy.criteria {
        *item = protect_text(item, &finding.id, keep_paths, warnings);
    }
    copy
}

fn is_id_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// The byte ranges where `id` appears as a whole word (not inside a longer id), ignoring case
/// like the script's `-match`/`-replace`.
fn id_spans(text: &str, id: &str) -> Vec<std::ops::Range<usize>> {
    let pattern = re(&format!("(?i){}", regex::escape(id)));
    pattern
        .find_iter(text)
        .filter(|m| {
            let before = text[..m.start()].chars().next_back();
            let after = text[m.end()..].chars().next();
            !before.is_some_and(is_id_char) && !after.is_some_and(is_id_char)
        })
        .map(|m| m.range())
        .collect()
}

fn mentions(text: &str, id: &str) -> bool {
    !id_spans(text, id).is_empty()
}

/// Finding ids in a Depends on / Related text become issue numbers where they are known.
pub fn link_ids(text: &str, ids: &BTreeMap<String, u32>) -> String {
    let mut text = text.to_owned();
    for (id, number) in ids {
        for span in id_spans(&text, id).into_iter().rev() {
            text.replace_range(span, &format!("#{number}"));
        }
    }
    text
}

/// An existing issue's body with finding ids rewritten to `#n` inside its `Depends on` and
/// `Related` fields, and nothing else, so hand edits elsewhere stay. A field runs to the next
/// bold field, comment or blank line.
pub fn update_link_fields(body: &str, ids: &BTreeMap<String, u32>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut in_field = false;
    for raw in body.split_inclusive('\n') {
        let line = raw.trim_end_matches('\n');
        let content = line.strip_suffix('\r').unwrap_or(line);
        let start = content.starts_with("**Depends on:**") || content.starts_with("**Related:**");
        if start {
            in_field = true;
        } else if content.is_empty() || content.starts_with("**") || content.starts_with("<!--") {
            in_field = false;
        }
        if in_field {
            out.push_str(&link_ids(content, ids));
            out.push_str(&raw[content.len()..]);
        } else {
            out.push_str(raw);
        }
    }
    out
}

/// `src/a.rs:10-20` or `justfile:76` becomes a permalink at the reviewed commit. Plain words are
/// left alone. Without a sha the text stays and `warnings` says so.
pub fn where_links(
    text: &str,
    repo: &str,
    sha: Option<&str>,
    warnings: &mut Vec<String>,
) -> String {
    // Extensionless names count only when they are a well-known file name and carry a line
    // suffix, so words such as "step:3" or "ratio 3:1" stay text.
    static LINK: LazyLock<Regex> = LazyLock::new(|| {
        re(concat!(
            r"(`?)((?:[\w.-]+/)*)(?:([\w.-]+\.\w+)(?::(\d+)(?:-(\d+))?)?",
            r"|((?:justfile|Dockerfile|Containerfile|Makefile|Rakefile|Gemfile|Procfile|Brewfile|Vagrantfile|Jenkinsfile|LICENSE)):(\d+)(?:-(\d+))?)(`?)"
        ))
    });
    static KNOWN: LazyLock<Regex> =
        LazyLock::new(|| re(r"(?i)\.(rs|toml|md|ps1|psm1|json|ya?ml|lock|txt|sh|py|js|ts)$"));
    let mut out = String::new();
    let mut pos = 0;
    let mut at = 0;
    while at <= text.len() {
        let Some(c) = LINK.captures_at(text, at) else {
            break;
        };
        let whole = c.get(0).expect("group 0");
        // The match may not follow a word character, `/`, `.`, `:` or `-` (no look-behind in `regex`).
        let before = text[..whole.start()].chars().next_back();
        if before.is_some_and(|b| b.is_alphanumeric() || "_/.:-".contains(b)) {
            at = whole.start()
                + text[whole.start()..]
                    .chars()
                    .next()
                    .map_or(1, char::len_utf8);
            continue;
        }
        let name = c.get(3).or_else(|| c.get(6)).map_or("", |m| m.as_str());
        let path = format!("{}{name}", c.get(2).map_or("", |m| m.as_str()));
        let (from, to) = match c.get(3) {
            Some(_) => (c.get(4), c.get(5)),
            None => (c.get(7), c.get(8)),
        };
        let from = from.map_or("", |m| m.as_str());
        let to = to.map_or("", |m| m.as_str());
        at = whole.end().max(whole.start() + 1);
        if !(KNOWN.is_match(&path) || path.contains('/') || !from.is_empty()) {
            continue;
        }
        let Some(sha) = sha else {
            warnings.push(
                "no commit sha found in the Scope line; Where links stay plain text".to_owned(),
            );
            return text.to_owned();
        };
        let mut label = path.clone();
        if !from.is_empty() {
            label.push_str(&format!(":{from}"));
        }
        if !to.is_empty() {
            label.push_str(&format!("-{to}"));
        }
        let anchor = if !to.is_empty() {
            format!("#L{from}-L{to}")
        } else if !from.is_empty() {
            format!("#L{from}")
        } else {
            String::new()
        };
        let url = format!("https://github.com/{repo}/blob/{sha}/{path}{anchor}");
        out.push_str(&text[pos..whole.start()]);
        if c.get(1).is_some_and(|m| !m.as_str().is_empty()) {
            out.push_str(&format!("[`{label}`]({url})"));
        } else {
            out.push_str(&format!("[{label}]({url})"));
        }
        pos = whole.end();
    }
    out.push_str(&text[pos..]);
    out
}

fn format_field(label: &str, value: &str) -> String {
    if value.contains('\n') {
        format!("**{label}:**\n{value}")
    } else {
        format!("**{label}:** {value}")
    }
}

/// The standard criteria from the skill's template, each with a lower-case fragment that tells
/// whether a finding has it already.
const STANDARD_CHECKLIST: [(&str, &str); 3] = [
    (
        "fails before the fix",
        "A test covering it fails before the fix and passes after (name it, or say which file it goes in)",
    ),
    (
        "cargo fmt",
        "`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass",
    ),
    (
        "docs updated",
        "Docs updated where behavior users see changed (`README.md`), or \"no user-visible change\"",
    ),
];
const MISSING_CRITERION: &str =
    "<the specific check is missing from the review: add one before working this issue>";

/// The criteria an issue gets: the finding's own, plus the standard ones it lacks. A must-fix or
/// should-fix with none is an error unless `standard` says to file it with only the standard
/// ones. Questions get none.
pub fn acceptance_criteria(finding: &Finding, standard: bool) -> Result<Vec<String>, String> {
    if finding.label == "question" {
        return Ok(Vec::new());
    }
    if finding.criteria.is_empty() && !standard {
        return Err(format!(
            "{} has no acceptance criteria; add them to the review file, or rerun with --standard-criteria to file it with only the standard ones",
            finding.id
        ));
    }
    let mut items = Vec::new();
    if finding.criteria.is_empty() {
        items.push(MISSING_CRITERION.to_owned());
    }
    items.extend(finding.criteria.iter().cloned());
    for (fragment, text) in STANDARD_CHECKLIST {
        if !finding
            .criteria
            .iter()
            .any(|c| c.to_lowercase().contains(fragment))
        {
            items.push(text.to_owned());
        }
    }
    Ok(items)
}

/// What an issue body needs besides the finding.
pub struct Render<'a> {
    /// `owner/name`, for permalinks.
    pub repo: &'a str,
    /// The review's name in the marker (`review-small.md`).
    pub review_name: &'a str,
    /// What the `Review:` field names (`sdlc/reviews/review-small.md`).
    pub review_ref: &'a str,
    /// The reviewed commit, full; none leaves `Where` as text.
    pub head_sha: Option<&'a str>,
    /// Issue numbers known so far, by finding id.
    pub ids: &'a BTreeMap<String, u32>,
    pub standard_criteria: bool,
    /// The PR this review covers; written as the body's first line, `PR: #n` (decision 169).
    /// `None` when the review isn't tied to a PR.
    pub pr: Option<u32>,
}

/// The hidden marker that says which finding of which review an issue is.
pub fn marker(review_name: &str, id: &str) -> String {
    format!("<!-- review-finding: {review_name}#{id} -->")
}

/// The PR a finding issue's body names, read from its first line only (decision 169): `None` for
/// a line anywhere else, or no such line at all.
pub fn pr_of(body: &str) -> Option<u32> {
    static PR: LazyLock<Regex> = LazyLock::new(|| re(r"^PR: #(\d+)$"));
    let first = *lines_of(body).first()?;
    PR.captures(first)?[1].parse().ok()
}

/// The issue text for one finding: the skill's template in order, the marker on the last line.
pub fn issue_body(
    review: &Review,
    finding: &Finding,
    render: &Render,
    warnings: &mut Vec<String>,
) -> Result<String, String> {
    let criteria = acceptance_criteria(finding, render.standard_criteria)?;
    let is_question = finding.label == "question";
    let depends = finding.field("depends on").unwrap_or("none known");
    let mut related: Vec<String> = finding
        .field("related")
        .map(str::to_owned)
        .into_iter()
        .collect();
    related.extend(
        review
            .findings
            .iter()
            .filter(|f| {
                f.id != finding.id
                    && f.field("depends on")
                        .is_some_and(|d| mentions(d, &finding.id))
            })
            .map(|f| format!("{} depends on this one", f.id)),
    );
    let field = |name: &str| finding.field(name).unwrap_or("");

    let mut parts = Vec::new();
    if let Some(pr) = render.pr {
        parts.push(format!("PR: #{pr}"));
    }
    parts.extend([
        format_field(
            "Where",
            &where_links(field("where"), render.repo, render.head_sha, warnings),
        ),
        format_field("What happens", field("what happens")),
        format_field("Why it matters", field("why it matters")),
        format_field(
            if is_question {
                "Options and recommendation"
            } else {
                "Fix"
            },
            field("fix"),
        ),
        format_field("Depends on", &link_ids(depends, render.ids)),
    ]);
    if !related.is_empty() {
        parts.push(format_field(
            "Related",
            &link_ids(&related.join("; "), render.ids),
        ));
    }
    if !is_question {
        let items: Vec<String> = criteria.iter().map(|c| format!("- [ ] {c}")).collect();
        parts.push(format!("**Acceptance criteria:**\n{}", items.join("\n")));
    }
    let range = review
        .range
        .as_deref()
        .or(review.scope.as_deref())
        .unwrap_or("");
    parts.push(format!(
        "**Review:** `{}`, finding {}, reviewed commits `{range}`",
        render.review_ref, finding.id
    ));
    parts.push(marker(render.review_name, &finding.id));
    Ok(format!("{}\n", parts.join("\n")))
}

/// Findings in an order where each comes after the ones it depends on (file order otherwise; a
/// cycle keeps file order).
pub fn filing_order(findings: &[Finding]) -> Vec<&Finding> {
    let mut left: Vec<&Finding> = findings.iter().collect();
    let mut ordered = Vec::new();
    while !left.is_empty() {
        let ready = left
            .iter()
            .position(|me| {
                !left.iter().any(|other| {
                    other.id != me.id
                        && me
                            .field("depends on")
                            .is_some_and(|d| mentions(d, &other.id))
                })
            })
            .unwrap_or(0);
        ordered.push(left.remove(ready));
    }
    ordered
}

/// `Issues: S-1 #33, S-2 pending`: a number where the finding has an issue, `unknown` where it
/// would get one (a dry run), `pending` for the rest.
pub fn issues_line(
    findings: &[Finding],
    numbers: &BTreeMap<String, u32>,
    would_file: &[String],
    unknown: &str,
) -> String {
    let items: Vec<String> = findings
        .iter()
        .map(|f| match numbers.get(&f.id) {
            Some(n) => format!("{} #{n}", f.id),
            None if would_file.contains(&f.id) => format!("{} {unknown}", f.id),
            None => format!("{} pending", f.id),
        })
        .collect();
    format!("Issues: {}", items.join(", "))
}
