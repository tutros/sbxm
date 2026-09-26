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
    /// `None` when sbxm has no state for the sandbox.
    pub config: Option<ConfigStatus>,
}

/// Whether the config a sandbox was built from still matches (decision 55).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigStatus {
    Current,
    /// Changed since creation, or created before sbxm recorded hashes.
    Changed,
    /// The profile doesn't load, so the hash can't be computed.
    Unknown,
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
    let mut known: Vec<(String, String, String, ConfigStatus)> = Vec::new();
    for (project, state) in state::load_all(&config.base_dir)? {
        for (harness, sandbox) in state.sandboxes {
            let status = config_status(&config, &project, &sandbox);
            known.push((project.clone(), harness, sandbox.sandbox, status));
        }
    }

    let mut entries = Vec::new();
    for sandbox in backend.list()? {
        let Some((project, harness)) = project::parse_sandbox_name(&sandbox.name) else {
            continue;
        };
        let config = known
            .iter()
            .find(|(_, _, name, _)| *name == sandbox.name)
            .map(|(_, _, _, status)| *status);
        entries.push(Entry {
            project,
            harness,
            sandbox: sandbox.name,
            status: sandbox.status,
            problem: config.is_none().then_some(Problem::NoState),
            config,
        });
    }
    for (project, harness, sandbox, config) in known {
        if !entries.iter().any(|e| e.sandbox == sandbox) {
            entries.push(Entry {
                project,
                harness,
                sandbox,
                status: "missing".into(),
                problem: Some(Problem::NoSandbox),
                config: Some(config),
            });
        }
    }
    entries.sort_by(|a, b| (&a.project, &a.harness).cmp(&(&b.project, &b.harness)));
    Ok(entries)
}

/// A profile or project config that doesn't load is `Unknown`, not an error,
/// so one broken file doesn't hide every other sandbox.
fn config_status(
    config: &GlobalConfig,
    project: &str,
    sandbox: &state::SandboxState,
) -> ConfigStatus {
    let Some(stored) = &sandbox.config_hash else {
        return ConfigStatus::Changed;
    };
    let profile = sandbox
        .profile
        .as_deref()
        .unwrap_or(&config.default_profile);
    match config.current_hash(project, profile) {
        Ok(current) if current == *stored => ConfigStatus::Current,
        Ok(_) => ConfigStatus::Changed,
        Err(_) => ConfigStatus::Unknown,
    }
}

/// Aligned columns; orphans and changed configs get a note saying how to fix
/// them.
pub fn render_table(entries: &[Entry]) -> String {
    if entries.is_empty() {
        return "No sbxm sandboxes.\n".into();
    }
    let rows: Vec<[String; 5]> = entries
        .iter()
        .map(|e| {
            [
                e.project.clone(),
                e.harness.clone(),
                e.status.clone(),
                e.config.map(ConfigStatus::label).unwrap_or_default().into(),
                note(e),
            ]
        })
        .collect();
    let header = ["PROJECT", "HARNESS", "STATUS", "CONFIG", "NOTE"].map(String::from);
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

impl ConfigStatus {
    fn label(self) -> &'static str {
        match self {
            ConfigStatus::Current => "current",
            ConfigStatus::Changed => "changed",
            ConfigStatus::Unknown => "unknown",
        }
    }
}

/// Orphan problems come first: a missing sandbox is recreated from the
/// current config anyway.
fn note(entry: &Entry) -> String {
    let project = &entry.project;
    match (entry.problem, entry.config) {
        (None, Some(ConfigStatus::Changed)) => {
            format!("config changed; `sbxm open {project} --rebuild` recreates it")
        }
        (None, Some(ConfigStatus::Unknown)) => {
            format!("its profile or sandbox.toml doesn't load; `sbxm open {project}` shows why")
        }
        (None, _) => String::new(),
        (Some(Problem::NoSandbox), _) => {
            format!("sandbox missing; `sbxm open {project}` recreates it")
        }
        (Some(Problem::NoState), _) => "no sbxm state; not created by sbxm here".into(),
    }
}

pub fn render_json(entries: &[Entry]) -> String {
    serde_json::to_string_pretty(entries).expect("entries serialize") + "\n"
}
