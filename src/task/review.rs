//! Reading a reviewer's `review.md` (spec §5.2): its first line says how many must-fix findings
//! there are, and a review without that line isn't used.

use super::findings;
use super::record::OpenFinding;

/// The count on the first line, which must be exactly `Must-fix findings: <count>` (trailing
/// spaces and a Windows line ending are fine); `None` for anything else.
pub fn must_fix_count(review: &str) -> Option<u32> {
    let review = review.strip_prefix('\u{feff}').unwrap_or(review);
    let first = review.lines().next()?;
    let digits = first.trim_end().strip_prefix("Must-fix findings: ")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// GitHub accepts at most 65,536 characters in a comment; the review is cut well below that.
pub(crate) const COMMENT_CAP: usize = 60_000;

/// An `@name` at the start of a word would notify that user or team; a zero-width space after the
/// `@` keeps the text readable and the notification away. An `@` inside a word (an e-mail
/// address) is left alone.
pub(crate) fn defang_mentions(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous: Option<char> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        let at_word_start = previous.is_none_or(|p| !p.is_alphanumeric());
        if c == '@' && at_word_start && chars.peek().is_some_and(|n| n.is_alphanumeric()) {
            out.push('\u{200b}');
        }
        previous = Some(c);
    }
    out
}

/// The comment posted on a pull request: a line naming the review, then the reviewer's text with
/// mentions neutralised, cut (with a note) if it is longer than GitHub accepts.
pub fn pr_comment(number: u32, review: &str) -> String {
    let defanged = defang_mentions(review);
    let (body, cut) = match defanged.char_indices().nth(COMMENT_CAP) {
        Some((at, _)) => (&defanged[..at], true),
        None => (defanged.as_str(), false),
    };
    let mut text = format!("sbxm review of PR #{number}\n\n{}\n", body.trim_end());
    if cut {
        text.push_str(&format!(
            "\n(sbxm cut this review at {COMMENT_CAP} characters; the whole text is review.md in \
             the task folder.)\n"
        ));
    }
    text
}

/// The text saved as `review.md`: the reviewer's own words under a line naming who wrote them.
pub fn with_header(harness: &str, model: Option<&str>, review: &str) -> String {
    format!(
        "Reviewer: {harness} ({})\n\n{review}",
        model.unwrap_or("default model")
    )
}

/// [`must_fix_count`] of a saved `review.md`, read past the [`with_header`] line (a file without
/// it is read as it is).
pub fn saved_must_fix_count(saved: &str) -> Option<u32> {
    let review = saved
        .strip_prefix("Reviewer: ")
        .and_then(|rest| rest.split_once("\n\n"))
        .map_or(saved, |(_, review)| review);
    must_fix_count(review)
}

/// The individual file paths named by a `Where:` value (spec §5.3, issue 119): each
/// backtick-delimited reference, with any trailing `:<line>` or `:<line>-<line>` stripped so two
/// findings in the same file match regardless of which lines they point at. A `Where:` may name
/// more than one file (`` `a.rs:10`, `b.rs:20` ``); any one of them shared with another finding
/// is enough to match.
fn where_paths(raw: &str) -> Vec<&str> {
    let raw = raw.trim();
    if raw.contains('`') {
        raw.split('`')
            .skip(1)
            .step_by(2)
            .map(strip_line_suffix)
            .filter(|p| !p.is_empty())
            .collect()
    } else if raw.is_empty() {
        Vec::new()
    } else {
        vec![strip_line_suffix(raw)]
    }
}

fn strip_line_suffix(raw: &str) -> &str {
    let s = raw.trim();
    match s.rsplit_once(':') {
        Some((path, suffix))
            if !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit() || b == b'-') =>
        {
            path
        }
        _ => s,
    }
}

fn shares_a_path(a: &str, b: &str) -> bool {
    let a = where_paths(a);
    where_paths(b).into_iter().any(|p| a.contains(&p))
}

/// How a must-fix finding's `Repeat of:` claim matched the must-fix findings of every review
/// round before it (spec §5.3, decisions 174(b), 177(e), 177(o)).
enum Claim {
    /// No `Repeat of:` line, or the literal `Repeat of: new`: an ordinary new finding.
    None,
    /// Names an id with no must-fix finding in any earlier round sharing a file: wrong, but
    /// still counts as new rather than stopping the task.
    Invalid(String),
    /// Validly names an earlier round's must-fix finding that shares a file.
    Valid,
}

fn classify(finding: &findings::Finding, earlier: &[findings::Review]) -> Claim {
    let Some(claimed_id) = finding.field("repeat of") else {
        return Claim::None;
    };
    if claimed_id == "new" {
        return Claim::None;
    }
    let valid = earlier.iter().any(|review| {
        review.findings.iter().any(|earlier_finding| {
            earlier_finding.label == "must-fix"
                && earlier_finding.id == claimed_id
                && match (finding.field("where"), earlier_finding.field("where")) {
                    (Some(a), Some(b)) => shares_a_path(a, b),
                    _ => false,
                }
        })
    });
    if valid {
        Claim::Valid
    } else {
        Claim::Invalid(claimed_id.to_owned())
    }
}

/// Every must-fix finding of `current`, classified against the must-fix findings of
/// `earlier_reviews` (every review round before this one, spec §5.3, decisions 174(b), 177(o)): a
/// narrow round between two full ones must not hide a finding that keeps coming back, so all
/// earlier rounds count, not only the one immediately before.
fn classify_current(current: &str, earlier_reviews: &[&str]) -> Vec<(String, Claim)> {
    let earlier: Vec<findings::Review> =
        earlier_reviews.iter().map(|r| findings::parse(r)).collect();
    findings::parse(current)
        .findings
        .into_iter()
        .filter(|f| f.label == "must-fix")
        .map(|f| {
            let claim = classify(&f, &earlier);
            (f.id, claim)
        })
        .collect()
}

/// The no-progress rule's matching (spec §5.3, decisions 174(b), 177(o)): whether `current`'s
/// review has a must-fix finding that validly claims `Repeat of: <id>` against any of
/// `earlier_reviews`' must-fix findings (one entry per review round before this one, oldest
/// first) — the id must name one of them, and the two must share a `Where:` file path (line
/// numbers ignored). A missing `Repeat of:` line, an id no earlier round has a must-fix finding
/// for, or a different file all count as new, so a wrong claim alone can never stop the task; the
/// same is true of a repeat claimed on a should-fix finding or a question, which never stop it.
pub fn repeats_a_must_fix_finding(current: &str, earlier_reviews: &[&str]) -> bool {
    classify_current(current, earlier_reviews)
        .iter()
        .any(|(_, claim)| matches!(claim, Claim::Valid))
}

/// One warning per must-fix finding of `current` whose `Repeat of:` claim is invalid (decision
/// 177(e)): it names an id no earlier round has a must-fix finding for, or one in a different
/// file. `Repeat of: new` and a missing marker are ordinary new findings and never warned about,
/// so a malformed reviewer claim isn't silently dropped when it's also why the no-progress rule
/// didn't stop the task.
pub fn invalid_repeat_claims(current: &str, earlier_reviews: &[&str]) -> Vec<String> {
    classify_current(current, earlier_reviews)
        .into_iter()
        .filter_map(|(id, claim)| match claim {
            Claim::Invalid(claimed_id) => Some(format!(
                "{id} claims \"Repeat of: {claimed_id}\", but no earlier must-fix finding \
                 {claimed_id} shares its file; counted as new"
            )),
            _ => None,
        })
        .collect()
}

/// The must-fix findings of `review` (a reviewer's text, or a saved `review.md`), as found by
/// review round `round`.
pub fn must_fix_findings(review: &str, round: u32) -> Vec<OpenFinding> {
    findings::parse(review)
        .findings
        .into_iter()
        .filter(|f| f.label == "must-fix")
        .map(|f| OpenFinding {
            round,
            place: f.field("where").map(str::to_owned),
            id: f.id,
            title: f.title,
        })
        .collect()
}

/// The must-fix findings open once review round `round` (`current`, the reviewer's text) is
/// accepted (decision 177(p)): a `full` review saw every commit, so its findings are all that is
/// open; a narrow one saw only the latest, so the `earlier` open findings stay, except one it
/// names in a `Repeat of:` claim on the same file, which its own finding replaces.
pub fn open_after(
    earlier: Option<&[OpenFinding]>,
    current: &str,
    round: u32,
    full: bool,
) -> Vec<OpenFinding> {
    let found: Vec<findings::Finding> = findings::parse(current)
        .findings
        .into_iter()
        .filter(|f| f.label == "must-fix")
        .collect();
    let mut open: Vec<OpenFinding> = if full {
        Vec::new()
    } else {
        earlier
            .unwrap_or_default()
            .iter()
            .filter(|old| {
                !found.iter().any(|f| {
                    f.field("repeat of") == Some(old.id.as_str())
                        && match (f.field("where"), old.place.as_deref()) {
                            (Some(a), Some(b)) => shares_a_path(a, b),
                            _ => false,
                        }
                })
            })
            .cloned()
            .collect()
    };
    open.extend(must_fix_findings(current, round));
    open
}
