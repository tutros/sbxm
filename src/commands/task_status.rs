//! `sbxm task status`: one line per task from its `task.json` (spec §4). Reads only.

use std::fmt::Write;
use std::path::Path;

use anyhow::{Result, bail};
use serde_json::Value;

use crate::task::record::{self, Kind, ProcessProbe, Record};

fn id_of(kind: Kind, number: u32) -> String {
    match kind {
        Kind::Issue => format!("issue-{number}"),
        Kind::Pr => format!("pr-{number}"),
    }
}

fn yes_no(present: bool) -> &'static str {
    if present { "yes" } else { "no" }
}

/// The task branch's commits ahead of its base in the task's own `repo.git`; `None` while the
/// repo or the branch doesn't exist yet (or can't be read).
fn commits_ahead(base: &Path, record: &Record) -> Option<u32> {
    let repo_git = record::task_dir(base, &record.id).join("repo.git");
    if !repo_git.is_dir() {
        return None;
    }
    record.commits_ahead(&repo_git).ok()
}

/// `which`: show just that task (an error when it doesn't exist).
pub fn render(
    base: &Path,
    which: Option<(Kind, u32)>,
    json: bool,
    probe: &dyn ProcessProbe,
) -> Result<String> {
    let mut records = record::load_all(base)?;
    if let Some((kind, number)) = which {
        let id = id_of(kind, number);
        records.retain(|r| r.id == id);
        if records.is_empty() {
            bail!("no task {id}; run `sbxm task status` to list the tasks");
        }
    }
    if json {
        return render_json(base, &records, probe);
    }
    if records.is_empty() {
        return Ok("No tasks yet; start one with `sbxm task start --issue <n>`.\n".to_owned());
    }
    let width = records.iter().map(|r| r.id.len()).max().unwrap_or(0);
    let mut out = String::new();
    for record in &records {
        let dir = record::task_dir(base, &record.id);
        let status = if record.is_interrupted(probe) {
            "interrupted"
        } else {
            record.status.name()
        };
        writeln!(
            out,
            "{:<width$}  {:<9}  {:<12}  ahead: {:<3}  result: {:<3}  review: {:<3}  {}{}",
            record.id,
            record.stage.name(),
            status,
            commits_ahead(base, record).map_or_else(|| "-".to_owned(), |n| n.to_string()),
            yes_no(dir.join("result.md").is_file()),
            yes_no(dir.join("review.md").is_file()),
            record.title,
            record
                .continues
                .as_ref()
                .map_or_else(String::new, |c| format!("  [PR #{} ({})]", c.pr, c.branch)),
        )?;
    }
    Ok(out)
}

fn render_json(base: &Path, records: &[Record], probe: &dyn ProcessProbe) -> Result<String> {
    let values: Vec<Value> = records
        .iter()
        .map(|record| {
            let mut value = serde_json::to_value(record)?;
            value["interrupted"] = Value::Bool(record.is_interrupted(probe));
            value["commits_ahead"] = commits_ahead(base, record).map_or(Value::Null, Value::from);
            Ok(value)
        })
        .collect::<Result<_>>()?;
    Ok(format!("{}\n", serde_json::to_string_pretty(&values)?))
}
