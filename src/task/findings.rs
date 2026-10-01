//! The findings of a review file (spec §11, decisions 134, 149): parse `sdlc/reviews/*.md` or a
//! task's `review.md`, and later render, check and file them. Ported from
//! `scripts/file-review-issues.psm1`; its Pester cases are the golden tests.

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
