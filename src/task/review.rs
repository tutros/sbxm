//! Reading a reviewer's `review.md` (spec §5.2): its first line says how many must-fix findings
//! there are, and a review without that line isn't used.

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

/// The text saved as `review.md`: the reviewer's own words under a line naming who wrote them.
pub fn with_header(harness: &str, model: Option<&str>, review: &str) -> String {
    format!(
        "Reviewer: {harness} ({})\n\n{review}",
        model.unwrap_or("default model")
    )
}
