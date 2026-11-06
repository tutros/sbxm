//! The short title sbxm shows with a task, an issue or a pull request (issue 246): the issue's or
//! PR's title from GitHub as the task's record stored it, or a spec task's first `# ` heading
//! ([`crate::task::fixes::title`], the same as its PR's). Every
//! output that names one goes through [`titled`] (an item that heads its line or cell) or
//! [`named`] (an item inside a sentence), so a record without a title shows the number alone
//! everywhere alike. Reads only the task records; never calls GitHub.

use std::path::Path;

use crate::task::record::{self, Kind, Record};

/// The title as one line: line breaks become spaces, and the ends are trimmed.
fn one_line(title: &str) -> String {
    title.replace(['\r', '\n'], " ").trim().to_owned()
}

/// `#41: Add titles` for `label` `#41`: the label, a colon and the title; the label alone for an
/// empty title (an older record has none).
pub fn titled(label: &str, title: &str) -> String {
    match one_line(title) {
        title if title.is_empty() => label.to_owned(),
        title => format!("{label}: {title}"),
    }
}

/// `PR #9 (Add titles)`: the form of [`titled`] for an item inside a sentence, where a second
/// colon would read as the end of the item; the label alone for an empty title.
pub fn named(label: &str, title: &str) -> String {
    match one_line(title) {
        title if title.is_empty() => label.to_owned(),
        title => format!("{label} ({title})"),
    }
}

/// The label of the GitHub item (or spec) a task is about: `#41` for an issue, `#9` for a PR or a
/// PR's fix task, and the task id for a spec task.
pub fn reference(record: &Record) -> String {
    match record.kind {
        Kind::Spec => record.id.clone(),
        Kind::Issue | Kind::Pr | Kind::Fix => format!("#{}", record.number),
    }
}

/// The title of PR `number` from the task records under `base`: the record of the task that
/// opened it (its recorded PR URL ends in `/pull/<number>`), else a PR task's or a fix task's of
/// that number. `None` when no record names it (its task was removed, decision 195) or the one
/// that does has no title.
pub fn pr_title(base: &Path, number: u32) -> Option<String> {
    let records = record::load_all(base).ok()?;
    let opened = records.iter().find(|r| {
        r.pr.as_deref()
            .and_then(crate::task::finish::pr_number)
            .is_some_and(|n| n == number)
    });
    let reviewed = || {
        records
            .iter()
            .find(|r| matches!(r.kind, Kind::Pr | Kind::Fix) && r.number == number)
    };
    opened
        .or_else(reviewed)
        .map(|r| one_line(&r.title))
        .filter(|t| !t.is_empty())
}

/// `PR #9 (Add titles)` with the title from [`pr_title`], or `PR #9` without one.
pub fn pr_named(base: &Path, number: u32) -> String {
    named(
        &format!("PR #{number}"),
        &pr_title(base, number).unwrap_or_default(),
    )
}
