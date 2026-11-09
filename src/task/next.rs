//! The flag that names a task on the command line again, and the command that continues it
//! (issue 512): one place for what five copies wrote by the task's kind, so a new kind or a new
//! command changes it once (issue 502 came from a copy left behind).

use super::record::{Kind, Record, Stage};

/// The flag that names a task of `kind` and `number` again: `--issue N`; `--pr N` for a PR task
/// and for a fix task, which names the PR's fix task of its current head (issue 485); a spec
/// task has no path recorded, so it gets a placeholder (as `task resume`'s own flag does).
pub fn flag(kind: Kind, number: u32) -> String {
    match kind {
        Kind::Issue => format!("--issue {number}"),
        Kind::Pr | Kind::Fix => format!("--pr {number}"),
        Kind::Spec => "--spec <file>".to_owned(),
    }
}

/// [`flag`] of `record`.
pub fn record_flag(record: &Record) -> String {
    flag(record.kind, record.number)
}

/// The command that continues `record`'s failed, stopped or interrupted stage: `task resume`
/// for an issue, spec or fix task (issues 118, 485); a PR task has no resume and goes on with
/// `task review --pr N` (decision 282(a)).
pub fn resume(record: &Record) -> String {
    match record.kind {
        Kind::Pr => format!("sbxm task review {}", record_flag(record)),
        _ => format!("sbxm task resume {}", record_flag(record)),
    }
}

/// The next step of a task its review left `ready`, with `flag` naming it (the real spec path
/// where the caller has one, else [`record_flag`]); `None` at any other stage.
pub fn ready(record: &Record, flag: &str) -> Option<String> {
    if record.stage != Stage::Ready {
        return None;
    }
    let must_fix = record.review.as_ref().map_or(0, |r| r.must_fix);
    Some(match must_fix {
        0 => format!("sbxm task finish {flag}"),
        _ => format!("fix them by hand, or file them: sbxm task file-findings {flag}"),
    })
}
