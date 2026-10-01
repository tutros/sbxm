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
        return render_json(&records, probe);
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
            "{:<width$}  {:<9}  {:<12}  result: {:<3}  review: {:<3}  {}",
            record.id,
            record.stage.name(),
            status,
            yes_no(dir.join("result.md").is_file()),
            yes_no(dir.join("review.md").is_file()),
            record.title,
        )?;
    }
    Ok(out)
}

fn render_json(records: &[Record], probe: &dyn ProcessProbe) -> Result<String> {
    let values: Vec<Value> = records
        .iter()
        .map(|record| {
            let mut value = serde_json::to_value(record)?;
            value["interrupted"] = Value::Bool(record.is_interrupted(probe));
            Ok(value)
        })
        .collect::<Result<_>>()?;
    Ok(format!("{}\n", serde_json::to_string_pretty(&values)?))
}
