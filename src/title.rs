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

/// The title of PR `number` of `repo` from the task records under `base` (PR numbers are per
/// repository, so a record of another `repo` never counts): the record of the issue or spec task
/// that opened it (its recorded PR URL ends in `/pull/<number>`; a task that only continued it,
/// decision 169, didn't open it, and its title is its own issue's), else a PR task's or a fix
/// task's of that number. Each record is read on its own, as `queue::loop_prs` reads them: a
/// folder that isn't sbxm's own or a record that can't be read is skipped, never the whole
/// lookup. `None` when no record names it (its task was removed, decision 195) or the one that
/// does has no title. Among several openers (which sbxm never makes) the lowest id wins.
pub fn pr_title(base: &Path, repo: &str, number: u32) -> Option<String> {
    let mut opened: Option<(String, String)> = None;
    let mut reviewed: Option<(String, String)> = None;
    for dir in record::task_folders(base).ok()? {
        if crate::task::finish::check_own_folder(base, &dir).is_err() {
            continue;
        }
        let Ok(rec) = record::read(&dir.join("task.json")) else {
            continue;
        };
        let title = one_line(&rec.title);
        if rec.repo != repo || title.is_empty() {
            continue;
        }
        let slot = match rec.kind {
            Kind::Issue | Kind::Spec
                if rec.continues.is_none()
                    && rec.pr.as_deref().and_then(crate::task::finish::pr_number)
                        == Some(number) =>
            {
                &mut opened
            }
            Kind::Pr | Kind::Fix if rec.number == number => &mut reviewed,
            _ => continue,
        };
        if slot.as_ref().is_none_or(|(id, _)| rec.id < *id) {
            *slot = Some((rec.id, title));
        }
    }
    opened.or(reviewed).map(|(_, title)| title)
}

/// `PR #9 (Add titles)` with the title from [`pr_title`], or `PR #9` without one.
pub fn pr_named(base: &Path, repo: &str, number: u32) -> String {
    named(
        &format!("PR #{number}"),
        &pr_title(base, repo, number).unwrap_or_default(),
    )
}

/// What a `next:` hint ends with to say which task it is for (issue 246): two spaces and the
/// task's item with its title as a shell comment (`  # #41: Add titles`), which bash and
/// PowerShell alike ignore, so the line still runs when copied whole; empty without a title.
pub fn hint_comment(record: &Record) -> String {
    match one_line(&record.title) {
        title if title.is_empty() => String::new(),
        title => format!("  # {}", titled(&reference(record), &title)),
    }
}

/// [`hint_comment`] for the task `id` under `base`, read from its record; empty when the record
/// can't be read.
pub fn hint_comment_of(base: &Path, id: &str) -> String {
    record::read(&record::task_dir(base, id).join("task.json"))
        .map_or_else(|_| String::new(), |r| hint_comment(&r))
}
