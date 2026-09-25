use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::{project, state};

/// One row of `sbxm list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub project: String,
    pub harness: String,
    pub sandbox: String,
    pub status: String,
    pub problem: Option<Problem>,
}

/// An orphan: `sbx` and sbxm's state disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Problem {
    /// The sandbox exists but sbxm has no state for it.
    NoState,
    /// sbxm has state but the sandbox is gone.
    NoSandbox,
}

pub fn entries(config_dir: &Path, backend: &dyn SandboxBackend) -> Result<Vec<Entry>> {
    let config = GlobalConfig::load(config_dir)?;
    let mut known: Vec<(String, String, String)> = Vec::new();
    for (project, state) in state::load_all(&config.base_dir)? {
        for (harness, sandbox) in state.sandboxes {
            known.push((project.clone(), harness, sandbox.sandbox));
        }
    }

    let mut entries = Vec::new();
    for sandbox in backend.list()? {
        let Some((project, harness)) = project::parse_sandbox_name(&sandbox.name) else {
            continue;
        };
        let has_state = known.iter().any(|(_, _, name)| *name == sandbox.name);
        entries.push(Entry {
            project,
            harness,
            sandbox: sandbox.name,
            status: sandbox.status,
            problem: (!has_state).then_some(Problem::NoState),
        });
    }
    for (project, harness, sandbox) in known {
        if !entries.iter().any(|e| e.sandbox == sandbox) {
            entries.push(Entry {
                project,
                harness,
                sandbox,
                status: "missing".into(),
                problem: Some(Problem::NoSandbox),
            });
        }
    }
    entries.sort_by(|a, b| (&a.project, &a.harness).cmp(&(&b.project, &b.harness)));
    Ok(entries)
}

/// Aligned columns; orphan rows get a note saying how to fix them.
pub fn render_table(entries: &[Entry]) -> String {
    if entries.is_empty() {
        return "No sbxm sandboxes.\n".into();
    }
    let rows: Vec<[String; 4]> = entries
        .iter()
        .map(|e| {
            [
                e.project.clone(),
                e.harness.clone(),
                e.status.clone(),
                note(e),
            ]
        })
        .collect();
    let header = ["PROJECT", "HARNESS", "STATUS", "NOTE"].map(String::from);
    let mut widths = header.clone().map(|h| h.len());
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.len());
        }
    }
    let mut out = String::new();
    for row in std::iter::once(&header).chain(&rows) {
        let line: String = row
            .iter()
            .zip(widths)
            .map(|(cell, width)| format!("{cell:width$}  "))
            .collect();
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn note(entry: &Entry) -> String {
    match entry.problem {
        None => String::new(),
        Some(Problem::NoSandbox) => {
            format!(
                "sandbox missing; `sbxm open {}` recreates it",
                entry.project
            )
        }
        Some(Problem::NoState) => "no sbxm state; not created by sbxm here".into(),
    }
}

pub fn render_json(entries: &[Entry]) -> String {
    serde_json::to_string_pretty(entries).expect("entries serialize") + "\n"
}
