//! Reading a reviewer's `review.md` (spec §5.2): its first line says how many must-fix findings
//! there are, and a review without that line isn't used.

use super::findings;

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

/// A `Where:` value with any trailing `:<line>` or `:<line>-<line>` stripped, so two findings in
/// the same file match regardless of which lines they point at (spec §5.3, issue 119).
fn where_path(raw: &str) -> &str {
    let s = raw.trim().trim_matches('`');
    match s.rsplit_once(':') {
        Some((path, suffix))
            if !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit() || b == b'-') =>
        {
            path
        }
        _ => s,
    }
}

/// The no-progress rule's matching (spec §5.3, decisions 174(b), 177(e)): whether `current`'s
/// review has a must-fix finding that validly claims `Repeat of: <id>` against `previous`'s
/// must-fix findings — the id must name one of them, and the two must share a `Where:` file path
/// (line numbers ignored). A missing `Repeat of:` line, an id `previous` has no must-fix finding
/// for, or a different file all count as new, so a wrong claim alone can never stop the task; the
/// same is true of a repeat claimed on a should-fix finding or a question, which never stop it.
pub fn repeats_a_must_fix_finding(current: &str, previous: &str) -> bool {
    let previous = findings::parse(previous);
    findings::parse(current).findings.iter().any(|finding| {
        finding.label == "must-fix"
            && finding.field("repeat of").is_some_and(|claimed_id| {
                previous.findings.iter().any(|earlier| {
                    earlier.label == "must-fix"
                        && earlier.id == claimed_id
                        && match (finding.field("where"), earlier.field("where")) {
                            (Some(a), Some(b)) => where_path(a) == where_path(b),
                            _ => false,
                        }
                })
            })
    })
}
